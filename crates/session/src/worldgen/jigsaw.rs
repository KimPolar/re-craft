//! Prepare data-pack jigsaw starts with biome and base-terrain checks.
//!
//! These starts have not yet had processors applied, blocks placed, or save
//! data written. The chunk pipeline must not treat them as completed terrain.
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use native_core::beardifier::{PieceProjection, StructurePiece, TerrainAdjustment};
use native_persistence::jigsaw_assembly::TemplateAssemblyContext;
use native_resources::{
    height_provider::HeightContext,
    jigsaw_placement::{self, Piece, StartHeightmap},
    jigsaw_structure::JigsawStructure,
    packs::{ResourceId, ResourceProvider},
    structure_pool::{LiquidSettings, PoolElement, Projection},
    structure_start::BiomeQuery,
};
use serde_json::Value;

mod placement;
pub use placement::{JigsawPlacementWorld, StructurePlacementWorld, place_structure_records};

/// Sampling must use the unadjusted generator and the structure's biome rules.
pub trait Environment {
    fn dimension_bounds(&self) -> (i32, i32);
    fn height_context(&self) -> HeightContext;
    fn biome_allowed(&mut self, quart: [i32; 3], query: BiomeQuery) -> Result<bool>;
    /// Java ChunkGenerator.getFirstFreeHeight, not first occupied height.
    fn first_free_height(&mut self, x: i32, z: i32, map: StartHeightmap) -> Result<i32>;
}

/// Complete piece graph before processors, block placement, and serialization.
#[derive(Clone, Debug)]
pub struct PreparedStart {
    pub chunk: [i32; 2],
    pub generation_point: [i32; 3],
    pub terrain_adjustment: TerrainAdjustment,
    pub liquid_settings: LiquidSettings,
    pub pieces: Vec<Piece>,
}

/// Processor lists aligned with a piece's pool-element tree. Feature elements
/// still require their own placement implementation.
#[derive(Clone, Debug)]
pub enum PieceProcessors {
    Empty,
    Feature,
    Single(Arc<native_persistence::structure_processors::PreparedProcessors>),
    List(Vec<PieceProcessors>),
}

#[derive(Clone)]
enum PlacementElement {
    Empty,
    Single(PreparedTemplatePiece),
    Feature {
        placed: Arc<super::ore::PlacedDecorationFeature>,
        origin: [i32; 3],
    },
    List(Vec<PlacementElement>),
}

pub(super) enum JigsawElement<'a> {
    Template(&'a PreparedTemplatePiece),
    Feature(&'a super::ore::PlacedDecorationFeature, [i32; 3]),
}

/// Prepared immutable start and its pieces, in the order used by Java.
#[derive(Clone)]
pub struct PreparedJigsawPlacement {
    source: PreparedStart,
    pieces: Vec<PlacementElement>,
}

impl PreparedJigsawPlacement {
    /// Preflight all currently supported elements. Templates referenced more
    /// than once share the same immutable data for the whole start.
    pub fn load(
        start: &PreparedStart,
        provider: &mut impl ResourceProvider,
        blocks: &BTreeSet<String>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<Self> {
        ensure!(!start.pieces.is_empty(), "jigsaw start has no pieces");
        let processors = start.prepare_processors(provider, blocks, registry)?;
        let mut templates = BTreeMap::new();
        let mut pieces = Vec::with_capacity(start.pieces.len());
        for (piece, processor) in start.pieces.iter().zip(&processors) {
            ensure!(
                (0..3).all(|i| piece.bounds.min[i] <= piece.bounds.max[i]),
                "invalid jigsaw piece bounds"
            );
            pieces.push(prepare_placement_element(
                piece,
                &piece.element,
                processor,
                provider,
                registry,
                &mut templates,
                0,
            )?);
        }
        Ok(Self {
            source: start.clone(),
            pieces,
        })
    }

    /// Verify that cached templates still belong to the exact retained start.
    pub(super) fn validate_source(&self, start: &PreparedStart) -> Result<()> {
        ensure!(
            self.source.chunk == start.chunk
                && self.source.generation_point == start.generation_point
                && self.source.terrain_adjustment == start.terrain_adjustment
                && self.source.liquid_settings == start.liquid_settings
                && self.source.pieces == start.pieces,
            "prepared jigsaw start changed"
        );
        ensure!(
            self.pieces.len() == start.pieces.len(),
            "prepared jigsaw piece count changed"
        );
        Ok(())
    }

    /// Process intersecting pieces in start order. The caller must stage both
    /// world and RNG until the complete decoration pass succeeds.
    pub(super) fn place_with(
        &self,
        start: &PreparedStart,
        clip: native_core::beardifier::BoundingBox,
        random: &mut native_core::random::DecorationRandom,
        mut place: impl FnMut(
            JigsawElement<'_>,
            LiquidSettings,
            &mut native_core::random::DecorationRandom,
        ) -> Result<bool>,
    ) -> Result<usize> {
        self.validate_source(start)?;
        ensure!(
            (0..3).all(|i| clip.min[i] <= clip.max[i]),
            "invalid jigsaw clipping box"
        );
        let mut next_random = random.clone();
        let mut count = 0;
        for (piece, prepared) in start.pieces.iter().zip(&self.pieces) {
            if !(0..3)
                .all(|i| piece.bounds.max[i] >= clip.min[i] && piece.bounds.min[i] <= clip.max[i])
            {
                continue;
            }
            let _ = prepared.place(start.liquid_settings, &mut next_random, &mut place)?;
            count += 1;
        }
        *random = next_random;
        Ok(count)
    }

    pub(super) fn features(&self) -> Vec<&super::ore::PlacedDecorationFeature> {
        let mut features = Vec::new();
        for piece in &self.pieces {
            piece.collect_features(&mut features);
        }
        features
    }
}

impl PlacementElement {
    fn collect_features<'a>(&'a self, features: &mut Vec<&'a super::ore::PlacedDecorationFeature>) {
        match self {
            Self::Feature { placed, .. } => features.push(placed),
            Self::List(elements) => {
                for element in elements {
                    element.collect_features(features);
                }
            }
            _ => {}
        }
    }
    fn place(
        &self,
        liquids: LiquidSettings,
        random: &mut native_core::random::DecorationRandom,
        place: &mut impl FnMut(
            JigsawElement<'_>,
            LiquidSettings,
            &mut native_core::random::DecorationRandom,
        ) -> Result<bool>,
    ) -> Result<bool> {
        match self {
            Self::Empty => Ok(true),
            Self::Single(piece) => place(JigsawElement::Template(piece), liquids, random),
            Self::Feature { placed, origin } => {
                place(JigsawElement::Feature(placed, *origin), liquids, random)
            }
            Self::List(elements) => {
                for element in elements {
                    if !element.place(liquids, random, place)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }
}

fn prepare_placement_element(
    piece: &Piece,
    element: &PoolElement,
    processors: &PieceProcessors,
    provider: &mut impl ResourceProvider,
    registry: &native_resources::block_registry::GeneratedBlockRegistry,
    templates: &mut BTreeMap<
        ResourceId,
        Arc<native_persistence::structure_template::StructureTemplate>,
    >,
    depth: usize,
) -> Result<PlacementElement> {
    ensure!(depth <= 64, "jigsaw element nesting too deep");
    Ok(match (element, processors) {
        (PoolElement::Empty, PieceProcessors::Empty) => PlacementElement::Empty,
        (PoolElement::Single { .. }, PieceProcessors::Single(_)) => {
            let mut child = piece.clone();
            child.element = element.clone();
            PlacementElement::Single(PreparedTemplatePiece::load_cached(
                &child, processors, provider, templates,
            )?)
        }
        (
            PoolElement::List {
                elements,
                projection,
            },
            PieceProcessors::List(prepared),
        ) => {
            ensure!(
                !elements.is_empty() && elements.len() == prepared.len(),
                "invalid jigsaw element list"
            );
            for element in elements {
                let actual = match element {
                    PoolElement::Empty => continue,
                    PoolElement::Single { projection, .. }
                    | PoolElement::Feature { projection, .. }
                    | PoolElement::List { projection, .. } => projection,
                };
                ensure!(
                    actual == projection,
                    "jigsaw list element projection differs"
                );
            }
            PlacementElement::List(
                elements
                    .iter()
                    .zip(prepared)
                    .map(|(element, processor)| {
                        prepare_placement_element(
                            piece,
                            element,
                            processor,
                            provider,
                            registry,
                            templates,
                            depth + 1,
                        )
                    })
                    .collect::<Result<Vec<_>>>()?,
            )
        }
        (PoolElement::Feature { feature, .. }, PieceProcessors::Feature) => {
            let mut resolve = |value: &Value| {
                let (name, properties) = registry.resolve(value)?;
                Ok(native_world::loaded::BlockState { name, properties })
            };
            let placed = match feature {
                native_resources::structure_pool::HolderValue::Reference(id) => {
                    super::ore::PlacedDecorationFeature::load(provider, id, &mut resolve)?
                }
                native_resources::structure_pool::HolderValue::Inline(value) => {
                    super::ore::PlacedDecorationFeature::from_placement(
                        provider,
                        native_resources::placed_feature::PlacedFeature::decode(value)?,
                        &mut resolve,
                    )?
                }
            };
            PlacementElement::Feature {
                placed: Arc::new(placed),
                origin: piece.origin,
            }
        }
        _ => anyhow::bail!("jigsaw processors do not match their element"),
    })
}

/// A single-template piece with its active data and processors. This is
/// still an intermediate result; placement and entities remain separate.
#[derive(Clone)]
pub struct PreparedTemplatePiece {
    template: Arc<native_persistence::structure_template::StructureTemplate>,
    processors: Arc<native_persistence::structure_processors::PreparedProcessors>,
    origin: [i32; 3],
    rotation: native_resources::structure_transform::Rotation,
    projection: Projection,
    legacy: bool,
    override_liquid_settings: Option<LiquidSettings>,
}

impl PreparedTemplatePiece {
    pub fn load(
        piece: &Piece,
        processors: &PieceProcessors,
        provider: &mut impl ResourceProvider,
    ) -> Result<Self> {
        Self::load_cached(piece, processors, provider, &mut BTreeMap::new())
    }

    fn load_cached(
        piece: &Piece,
        processors: &PieceProcessors,
        provider: &mut impl ResourceProvider,
        cache: &mut BTreeMap<
            ResourceId,
            Arc<native_persistence::structure_template::StructureTemplate>,
        >,
    ) -> Result<Self> {
        let PoolElement::Single {
            location,
            legacy,
            projection,
            override_liquid_settings,
            ..
        } = &piece.element
        else {
            anyhow::bail!("jigsaw piece is not a single template");
        };
        let PieceProcessors::Single(processors) = processors else {
            anyhow::bail!("jigsaw piece processors do not match its element");
        };
        let template = if let Some(template) = cache.get(location) {
            Arc::clone(template)
        } else {
            let template = Arc::new(
                native_persistence::structure_template::StructureTemplate::load(
                    provider, location,
                )?
                .with_context(|| {
                    format!(
                        "missing jigsaw template {}:{}",
                        location.namespace, location.path
                    )
                })?,
            );
            cache.insert(location.clone(), Arc::clone(&template));
            template
        };
        ensure!(
            template.data_version == Some(5023),
            "jigsaw template needs unsupported data-version conversion"
        );
        Ok(Self {
            template,
            processors: Arc::clone(processors),
            origin: piece.origin,
            rotation: piece.rotation,
            projection: *projection,
            legacy: *legacy,
            override_liquid_settings: *override_liquid_settings,
        })
    }

    /// Return unrotated, processed records in Java order without world writes.
    pub fn process_blocks(
        &self,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        clip: native_core::beardifier::BoundingBox,
        is_static_full: impl FnMut(&native_world::loaded::BlockState) -> Result<bool>,
        world: &mut impl FnMut([i32; 3]) -> Result<native_world::loaded::BlockState>,
    ) -> Result<Vec<native_persistence::jigsaw_template::ProcessedTemplateBlock>> {
        self.process_blocks_with_height(registry, clip, is_static_full, world, &mut |_, _| {
            anyhow::bail!("terrain-matching jigsaw heightmap is missing")
        })
    }

    fn process_blocks_with_height(
        &self,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        clip: native_core::beardifier::BoundingBox,
        is_static_full: impl FnMut(&native_world::loaded::BlockState) -> Result<bool>,
        world: &mut impl FnMut([i32; 3]) -> Result<native_world::loaded::BlockState>,
        height: &mut impl FnMut(i32, i32) -> Result<i32>,
    ) -> Result<Vec<native_persistence::jigsaw_template::ProcessedTemplateBlock>> {
        use native_persistence::jigsaw_template::{
            TemplateSettings, process_template, resolve_template_state,
        };
        process_template(
            &self.template,
            registry,
            &self.processors,
            TemplateSettings {
                origin: self.origin,
                rotation: self.rotation,
                clip,
                legacy: self.legacy,
                keep_jigsaws: false,
            },
            |tag| resolve_template_state(tag, registry),
            is_static_full,
            world,
            |original, mut block| {
                if self.projection == Projection::TerrainMatching {
                    block.position[1] = height(block.position[0], block.position[2])?
                        .wrapping_sub(1)
                        .wrapping_add(original.position[1]);
                }
                Ok(Some(block))
            },
        )
    }
}

fn prepare_element_processors(
    element: &PoolElement,
    provider: &mut impl ResourceProvider,
    blocks: &BTreeSet<String>,
    registry: &native_resources::block_registry::GeneratedBlockRegistry,
    cache: &mut BTreeMap<
        ResourceId,
        Arc<native_persistence::structure_processors::PreparedProcessors>,
    >,
) -> Result<PieceProcessors> {
    use native_persistence::structure_processors::PreparedProcessors;
    use native_resources::structure_pool::HolderValue;
    Ok(match element {
        PoolElement::Empty => PieceProcessors::Empty,
        PoolElement::Feature { .. } => PieceProcessors::Feature,
        PoolElement::List { elements, .. } => PieceProcessors::List(
            elements
                .iter()
                .map(|element| {
                    prepare_element_processors(element, provider, blocks, registry, cache)
                })
                .collect::<Result<Vec<_>>>()?,
        ),
        PoolElement::Single { processors, .. } => {
            let prepared = match processors {
                HolderValue::Reference(id) => {
                    if let Some(prepared) = cache.get(id) {
                        Arc::clone(prepared)
                    } else {
                        let prepared = Arc::new(PreparedProcessors::load(
                            processors, provider, blocks, registry,
                        )?);
                        cache.insert(id.clone(), Arc::clone(&prepared));
                        prepared
                    }
                }
                HolderValue::Inline(_) => Arc::new(PreparedProcessors::load(
                    processors, provider, blocks, registry,
                )?),
            };
            PieceProcessors::Single(prepared)
        }
    })
}

struct StartAdapter<'a, E>(&'a RefCell<&'a mut E>);

impl<E: Environment> jigsaw_placement::StartEnvironment for StartAdapter<'_, E> {
    fn valid_column(&mut self, x: i32, z: i32, min_y: i32, max_y: i32) -> Result<bool> {
        let mut env = self
            .0
            .try_borrow_mut()
            .context("jigsaw environment is already borrowed")?;
        for y in min_y.div_euclid(4)..=max_y.div_euclid(4) {
            if env.biome_allowed([x.div_euclid(4), y, z.div_euclid(4)], BiomeQuery::Column)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn first_free_height(&mut self, x: i32, z: i32, map: StartHeightmap) -> Result<i32> {
        self.0
            .try_borrow_mut()
            .context("jigsaw environment is already borrowed")?
            .first_free_height(x, z, map)
    }
}

impl PreparedStart {
    /// Resolve each piece's active processor list once per referenced holder.
    /// This prepares decoration but does not authorize block or entity writes.
    pub fn prepare_processors(
        &self,
        provider: &mut impl ResourceProvider,
        blocks: &BTreeSet<String>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<Vec<PieceProcessors>> {
        let mut cache = BTreeMap::new();
        self.pieces
            .iter()
            .map(|piece| {
                prepare_element_processors(&piece.element, provider, blocks, registry, &mut cache)
            })
            .collect()
    }

    /// Construct a fresh start tag only when every piece is preservable.
    /// This does not authorize publishing it to a world.
    pub fn to_tag(&self, structure: &ResourceId) -> Result<native_persistence::nbt::Tag> {
        native_persistence::jigsaw_piece::SavedJigsawStart {
            structure: structure.clone(),
            chunk: self.chunk,
            references: 0,
            pieces: self
                .pieces
                .iter()
                .cloned()
                .map(|piece| native_persistence::jigsaw_piece::SavedJigsawPiece {
                    piece,
                    liquid_settings: self.liquid_settings,
                })
                .collect(),
        }
        .to_tag()
    }

    /// Generate only after the caller has selected a structure-set entry.
    /// A failed final biome test never executes the lazy piece assembler.
    pub fn generate(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        environment: &mut impl Environment,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<Option<Self>> {
        let settings = JigsawStructure::decode(definition)?;
        let (min_y, max_y) = environment.dimension_bounds();
        ensure!(
            min_y <= max_y && i64::from(max_y) - i64::from(min_y) <= 4096,
            "invalid dimension bounds"
        );
        let mut seeded = settings.begin(seed, chunk, environment.height_context())?;
        let shared = RefCell::new(environment);
        let mut assembly = TemplateAssemblyContext::new(
            provider,
            |position: [i32; 3]| {
                shared
                    .try_borrow_mut()
                    .context("jigsaw environment is already borrowed")?
                    .first_free_height(position[0], position[2], StartHeightmap::WorldSurfaceWg)
            },
            seeded.aliases.clone(),
        );
        let mut adapter = StartAdapter(&shared);
        let Some(start) = seeded.prepare(&settings, &mut assembly, &mut adapter, min_y, max_y)?
        else {
            return Ok(None);
        };
        let generation_point = start.position;
        if !shared
            .try_borrow_mut()
            .context("jigsaw environment is already borrowed")?
            .biome_allowed(generation_point.map(|v| v.div_euclid(4)), BiomeQuery::Point)?
        {
            return Ok(None);
        }
        let pieces = start.assemble(&mut assembly, &mut |bound| {
            seeded.random.next_int_bound(bound)
        })?;
        // Java marks starts with no pieces invalid, including configured depth zero.
        if pieces.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            chunk,
            generation_point,
            terrain_adjustment: settings.terrain_adjustment,
            liquid_settings: settings.liquid_settings,
            pieces,
        }))
    }

    pub fn reference_bounds(
        &self,
        structure: ResourceId,
    ) -> Result<super::structure_references::StructureStartBounds> {
        let bounds: Vec<_> = self.pieces.iter().map(|piece| piece.bounds).collect();
        super::structure_references::StructureStartBounds::from_pieces(
            structure,
            &bounds,
            self.terrain_adjustment,
        )
    }

    /// Preserve piece and junction order for the terrain adaptation stage.
    pub fn terrain_pieces(&self) -> Result<Vec<StructurePiece>> {
        self.pieces
            .iter()
            .map(|piece| terrain_piece(piece, self.terrain_adjustment))
            .collect()
    }
}

/// Convert generated or restored pool pieces without changing junction order.
pub(super) fn terrain_piece(
    piece: &native_resources::jigsaw_placement::Piece,
    adjustment: TerrainAdjustment,
) -> Result<StructurePiece> {
    let projection = match &piece.element {
        PoolElement::Single { projection, .. }
        | PoolElement::Feature { projection, .. }
        | PoolElement::List { projection, .. } => *projection,
        PoolElement::Empty => anyhow::bail!("empty element cannot form a jigsaw piece"),
    };
    Ok(StructurePiece {
        bounds: piece.bounds,
        adjustment,
        ground_delta: piece.ground_level_delta,
        projection: match projection {
            Projection::Rigid => PieceProjection::Rigid,
            Projection::TerrainMatching => PieceProjection::TerrainMatching,
        },
        junctions: piece
            .junctions
            .iter()
            .map(|junction| native_core::beardifier::Junction {
                position: [
                    junction.source_x,
                    junction.source_ground_y,
                    junction.source_z,
                ],
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct TemplateOracleDraft {
        writes: Vec<([i32; 3], native_world::loaded::BlockState)>,
    }
    impl JigsawPlacementWorld for TemplateOracleDraft {
        fn block(&mut self, pos: [i32; 3]) -> Result<native_world::loaded::BlockState> {
            Ok(native_world::loaded::BlockState {
                name: if pos[0] % 7 == 0 {
                    "minecraft:bedrock"
                } else {
                    "minecraft:stone"
                }
                .into(),
                properties: BTreeMap::new(),
            })
        }
        fn world_surface_height(&mut self, x: i32, z: i32) -> Result<i32> {
            Ok(21 + (x + z).rem_euclid(5))
        }
        fn fluid_at(&mut self, _: [i32; 3]) -> Result<Option<native_world::fluid::FluidState>> {
            anyhow::bail!("unexpected fluid query")
        }
        fn fluid_of_state(
            &mut self,
            _: &native_world::loaded::BlockState,
        ) -> Result<Option<native_world::fluid::FluidState>> {
            anyhow::bail!("unexpected fluid query")
        }
        fn place_template_block(
            &mut self,
            pos: [i32; 3],
            state: &native_world::loaded::BlockState,
            nbt: Option<&native_persistence::nbt::Tag>,
            _: &mut native_core::random::DecorationRandom,
        ) -> Result<bool> {
            ensure!(
                nbt.is_none(),
                "oracle fixture unexpectedly has block entity data"
            );
            self.writes.push((pos, state.clone()));
            Ok(true)
        }
        fn is_liquid_container(&mut self, _: &native_world::loaded::BlockState) -> Result<bool> {
            anyhow::bail!("unexpected fluid query")
        }
        fn place_liquid(
            &mut self,
            _: [i32; 3],
            _: &native_world::loaded::BlockState,
            _: Option<native_world::fluid::FluidState>,
        ) -> Result<()> {
            anyhow::bail!("unexpected fluid placement")
        }
        fn mark_block_entity_changed(&mut self, _: [i32; 3]) -> Result<()> {
            anyhow::bail!("unexpected block entity")
        }
        fn place_template_entities(
            &mut self,
            _: &[native_persistence::nbt::Tag],
            _: [i32; 3],
            _: native_resources::structure_transform::Rotation,
            _: native_core::beardifier::BoundingBox,
        ) -> Result<()> {
            anyhow::bail!("unexpected template entities")
        }
    }

    #[test]
    #[ignore = "requires the installed verified 26.3 client and Java 25"]
    fn processed_real_template_matches_java() -> Result<()> {
        use native_persistence::structure_processors::PreparedProcessors;
        use native_resources::{structure_pool::HolderValue, structure_transform::Rotation};
        use native_world::loaded::BlockState;
        use std::{path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = data.vanilla_block_ids()?;
        let registry = data.vanilla_generated_block_registry()?;
        let holder = HolderValue::Inline(serde_json::json!({"processors":[
            {"processor_type":"minecraft:block_rot","integrity":0.83},
            {"processor_type":"minecraft:protected_blocks","value":["minecraft:bedrock"]}
        ]}));
        let prepared = PieceProcessors::Single(Arc::new(PreparedProcessors::load(
            &holder, &mut data, &blocks, &registry,
        )?));
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home is missing")?
            .join("bin");
        let source = temp.path().join("JigsawTemplateOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/JigsawTemplateOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawTemplateOracle")
            .arg(version.join("26.3.jar"))
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rotations = [
            Rotation::None,
            Rotation::Clockwise90,
            Rotation::Clockwise180,
            Rotation::Counterclockwise90,
        ];
        let mut expected = VecDeque::new();
        let mut expected_writes = VecDeque::new();
        let mut cases = 0;
        let mut checked = 0;
        let mut defaults_checked = false;
        let format_state = |state: &BlockState| {
            let mut actual = format!("Block{{{}}}", state.name);
            if !state.properties.is_empty() {
                let properties = state
                    .properties
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",");
                actual.push_str(&format!("[{properties}]"));
            }
            actual
        };
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["DEFAULT", structure, jigsaw] => {
                    for (id, expected) in [
                        ("minecraft:structure_block", *structure),
                        ("minecraft:jigsaw", *jigsaw),
                    ] {
                        let tag = native_persistence::nbt::Tag::Compound(vec![(
                            "id".into(),
                            native_persistence::nbt::Tag::String(id.into()),
                        )]);
                        let state = native_persistence::jigsaw_template::resolve_template_state(
                            &tag, &registry,
                        )?;
                        assert_eq!(format_state(&state), expected);
                    }
                    defaults_checked = true;
                }
                ["CASE", projection, rotation, clipped, count] => {
                    ensure!(expected.is_empty(), "previous case incomplete");
                    let rotation = rotations[rotation.parse::<usize>()?];
                    let clip = if *clipped == "0" {
                        native_core::beardifier::BoundingBox::new([-1000; 3], [1000; 3])
                            .map_err(anyhow::Error::msg)?
                    } else {
                        native_core::beardifier::BoundingBox::new([14, 20, -26], [19, 23, -20])
                            .map_err(anyhow::Error::msg)?
                    };
                    let piece = Piece {
                        element: PoolElement::Single {
                            legacy: false,
                            location: ResourceId::parse("minecraft:underwater_ruin/warm_1")?,
                            processors: holder.clone(),
                            projection: if *projection == "0" {
                                Projection::Rigid
                            } else {
                                Projection::TerrainMatching
                            },
                            override_liquid_settings: None,
                        },
                        origin: [17, 20, -23],
                        rotation,
                        bounds: clip,
                        ground_level_delta: 0,
                        junctions: Vec::new(),
                    };
                    let piece = PreparedTemplatePiece::load(&piece, &prepared, &mut data)?;
                    let result = piece.process_blocks_with_height(
                        &registry,
                        clip,
                        super::super::ocean_ruin::ocean_template_static_full,
                        &mut |pos| {
                            Ok(BlockState {
                                name: if pos[0] % 7 == 0 {
                                    "minecraft:bedrock"
                                } else {
                                    "minecraft:stone"
                                }
                                .into(),
                                properties: BTreeMap::new(),
                            })
                        },
                        &mut |x, z| Ok(21 + (x + z).rem_euclid(5)),
                    )?;
                    ensure!(
                        result.len() == count.parse::<usize>()?,
                        "different block count for {rotation:?} clip {clipped}"
                    );
                    expected.extend(result);
                    let mut draft = TemplateOracleDraft::default();
                    assert!(piece.place(
                        &mut draft,
                        &registry,
                        clip,
                        LiquidSettings::IgnoreWaterlogging,
                        super::super::ocean_ruin::ocean_template_static_full,
                        &mut native_core::random::DecorationRandom::new(77)
                    )?);
                    expected_writes.extend(draft.writes);
                    cases += 1;
                }
                ["BLOCK", x, y, z, state, nbt] => {
                    let block = expected.pop_front().context("unexpected Java block")?.block;
                    assert_eq!(
                        block.position,
                        [x.parse::<i32>()?, y.parse::<i32>()?, z.parse::<i32>()?]
                    );
                    assert_eq!(&format_state(&block.state), state);
                    assert_eq!(
                        if block.nbt.is_none() {
                            "none"
                        } else {
                            "present"
                        },
                        *nbt
                    );
                    checked += 1;
                }
                ["PLACE", placed, count] => {
                    assert_eq!(*placed, "true");
                    assert_eq!(expected_writes.len(), count.parse::<usize>()?);
                }
                ["WRITE", x, y, z, state, flags] => {
                    let (position, actual) = expected_writes
                        .pop_front()
                        .context("unexpected Java write")?;
                    assert_eq!(
                        position,
                        [x.parse::<i32>()?, y.parse::<i32>()?, z.parse::<i32>()?]
                    );
                    assert_eq!(&format_state(&actual), state);
                    assert_eq!(*flags, "18");
                }
                _ => {}
            }
        }
        ensure!(expected.is_empty(), "last case incomplete");
        ensure!(expected_writes.is_empty(), "last write case incomplete");
        assert!(defaults_checked);
        assert_eq!(cases, 16);
        assert!(checked > 100);
        Ok(())
    }

    struct Terrain {
        allow: bool,
        queries: Vec<(BiomeQuery, [i32; 3])>,
        heights: usize,
    }
    impl Environment for Terrain {
        fn dimension_bounds(&self) -> (i32, i32) {
            (-64, 319)
        }
        fn height_context(&self) -> HeightContext {
            HeightContext {
                min_y: 0,
                height: 0,
                sea_level: 0,
            }
        }
        fn biome_allowed(&mut self, quart: [i32; 3], query: BiomeQuery) -> Result<bool> {
            self.queries.push((query, quart));
            Ok(self.allow)
        }
        fn first_free_height(&mut self, _: i32, _: i32, _: StartHeightmap) -> Result<i32> {
            self.heights += 1;
            Ok(-27)
        }
    }

    #[test]
    fn column_precheck_uses_inclusive_quart_bounds_and_stops_on_match() -> Result<()> {
        use jigsaw_placement::StartEnvironment;
        let mut terrain = Terrain {
            allow: false,
            queries: vec![],
            heights: 0,
        };
        let shared = RefCell::new(&mut terrain);
        let mut adapter = StartAdapter(&shared);
        assert!(!adapter.valid_column(-1, -5, -5, 3)?);
        assert_eq!(
            shared.borrow().queries,
            vec![
                (BiomeQuery::Column, [-1, -2, -2]),
                (BiomeQuery::Column, [-1, -1, -2]),
                (BiomeQuery::Column, [-1, 0, -2])
            ]
        );
        shared.borrow_mut().allow = true;
        assert!(adapter.valid_column(-1, -5, -5, 3)?);
        assert_eq!(shared.borrow().queries.len(), 4);
        Ok(())
    }

    fn city_lines(
        resources: &mut native_resources::packs::ResourceStack,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<VecDeque<String>> {
        use native_resources::structure_transform::Rotation;
        let bytes = resources
            .read_data("data/minecraft/worldgen/structure/ancient_city.json")?
            .context("missing city definition")?;
        let mut definition: Value = serde_json::from_slice(&bytes)?;
        definition["size"] = serde_json::json!(2);
        let mut terrain = Terrain {
            allow: true,
            queries: vec![],
            heights: 0,
        };
        let start = PreparedStart::generate(resources, &definition, &mut terrain, seed, chunk)?
            .context("missing city")?;
        let saved = native_persistence::jigsaw_piece::SavedJigsawStart::from_tag(
            &start.to_tag(&ResourceId::parse("minecraft:ancient_city")?)?,
        )?;
        assert_eq!(saved.chunk, chunk);
        assert_eq!(saved.pieces.len(), start.pieces.len());
        assert_eq!(terrain.queries.len(), 1);
        assert_eq!(
            terrain.queries[0],
            (
                BiomeQuery::Point,
                start.generation_point.map(|n| n.div_euclid(4))
            )
        );
        let terrain_pieces = start.terrain_pieces()?;
        assert_eq!(terrain_pieces.len(), start.pieces.len());
        let bounds = start.reference_bounds(ResourceId::parse("minecraft:ancient_city")?)?;
        assert!(start.pieces.iter().all(|p| {
            (0..3).all(|i| {
                bounds.bounds.min[i] <= p.bounds.min[i] && bounds.bounds.max[i] >= p.bounds.max[i]
            })
        }));
        let point = start.generation_point;
        let mut result = VecDeque::new();
        result.push_back(format!(
            "C\t{seed}\t{}\t{}\t{}\t{}\t{}\t{}",
            chunk[0],
            chunk[1],
            point[0],
            point[1],
            point[2],
            start.pieces.len()
        ));
        for (piece, adapted) in start.pieces.into_iter().zip(terrain_pieces) {
            assert_eq!(piece.ground_level_delta, adapted.ground_delta);
            assert_eq!(piece.junctions.len(), adapted.junctions.len());
            let rotation = match piece.rotation {
                Rotation::None => 0,
                Rotation::Clockwise90 => 1,
                Rotation::Clockwise180 => 2,
                Rotation::Counterclockwise90 => 3,
            };
            result.push_back(format!(
                "P\t{rotation}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                piece.origin[0],
                piece.origin[1],
                piece.origin[2],
                piece.bounds.min[0],
                piece.bounds.min[1],
                piece.bounds.min[2],
                piece.bounds.max[0],
                piece.bounds.max[1],
                piece.bounds.max[2],
                piece.ground_level_delta,
                piece.junctions.len()
            ));
            for j in piece.junctions {
                let p = match j.destination_projection {
                    Projection::Rigid => "rigid",
                    Projection::TerrainMatching => "terrain_matching",
                };
                result.push_back(format!(
                    "J\t{}\t{}\t{}\t{}\t{p}",
                    j.source_x, j.source_ground_y, j.source_z, j.delta_y
                ));
            }
        }
        Ok(result)
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn session_city_preparation_matches_java_graph_and_biome_rejection() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let bytes = resources
            .read_data("data/minecraft/worldgen/structure/ancient_city.json")?
            .context("missing city definition")?;
        let mut definition: Value = serde_json::from_slice(&bytes)?;
        definition["size"] = serde_json::json!(2);
        let mut terrain = Terrain {
            allow: false,
            queries: vec![],
            heights: 0,
        };
        assert!(
            PreparedStart::generate(&mut resources, &definition, &mut terrain, 42, [0, 0])?
                .is_none()
        );
        assert_eq!(terrain.queries.len(), 1);
        assert_eq!(terrain.queries[0].0, BiomeQuery::Point);
        assert_eq!(terrain.heights, 0);
        definition["size"] = serde_json::json!(0);
        terrain.allow = true;
        assert!(
            PreparedStart::generate(&mut resources, &definition, &mut terrain, 42, [0, 0])?
                .is_none()
        );
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home missing")?
            .join("bin");
        let shared = temp.path().join("JigsawPlacementOracle.java");
        let source = temp.path().join("JigsawConfiguredCityOracle.java");
        std::fs::write(
            &shared,
            include_str!("../../../resources/tests/java/JigsawPlacementOracle.java"),
        )?;
        std::fs::write(
            &source,
            include_str!("../../../persistence/tests/java/JigsawConfiguredCityOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&shared)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawConfiguredCityOracle")
            .arg(version.join("26.3.jar"))
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let mut expected = VecDeque::new();
        let mut cases = 0;
        for line in String::from_utf8(output.stdout)?.lines() {
            if let Some(rest) = line.strip_prefix("C\t") {
                ensure!(expected.is_empty(), "previous graph incomplete");
                let fields: Vec<_> = rest.split('\t').collect();
                ensure!(fields.len() == 8, "invalid graph header");
                expected = city_lines(
                    &mut resources,
                    fields[0].parse()?,
                    [fields[1].parse()?, fields[2].parse()?],
                )?;
                let header = line.rsplit_once('\t').context("missing random state")?.0;
                assert_eq!(expected.pop_front().context("missing header")?, header);
                cases += 1;
            } else if line.starts_with("P\t") || line.starts_with("J\t") {
                assert_eq!(
                    expected.pop_front().context("extra graph entry")?,
                    line,
                    "case {cases}"
                );
            }
        }
        assert_eq!(cases, 6);
        assert!(expected.is_empty());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn terrain_generator_selects_and_prepares_a_jigsaw_start() -> Result<()> {
        use native_persistence::heightmap::BlockHeightFlags;
        use native_resources::{
            density::DensityEvaluator,
            structure_biomes::StructureBiomes,
            structure_placement::{StructureSetCache, StructureSetStart},
        };
        struct Overlay {
            inner: native_resources::packs::ResourceStack,
            definition: Vec<u8>,
        }
        impl ResourceProvider for Overlay {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.inner.read_asset(path)
            }
            fn read_builtin_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.inner.read_builtin_data(path)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/structure/ancient_city.json" {
                    Ok(Some(self.definition.clone()))
                } else {
                    self.inner.read_data(path)
                }
            }
            fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
                if path == "data/minecraft/worldgen/structure/ancient_city.json" {
                    Ok(vec![self.definition.clone()])
                } else {
                    self.inner.read_data_stack(path)
                }
            }
            fn list_data(&mut self, path: &str) -> Result<Vec<ResourceId>> {
                self.inner.list_data(path)
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let preset: Value = serde_json::from_slice(
            &resources
                .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")?
                .context("missing preset")?,
        )?;
        let generator = super::super::TerrainGenerator::with_structure_context(
            &mut resources,
            &preset["dimensions"]["minecraft:overworld"]["generator"],
            12345,
            -64,
            384,
        )?;
        let mut definition: Value = serde_json::from_slice(
            &resources
                .read_data("data/minecraft/worldgen/structure/ancient_city.json")?
                .context("missing city")?,
        )?;
        definition["size"] = serde_json::json!(2);
        definition["biomes"] = serde_json::Value::Array(
            generator
                .possible_biomes()
                .iter()
                .map(|id| serde_json::Value::String(format!("{}:{}", id.namespace, id.path)))
                .collect(),
        );
        let mut provider = Overlay {
            inner: resources,
            definition: serde_json::to_vec(&definition)?,
        };
        let structure = ResourceId::parse("minecraft:ancient_city")?;
        let set = ResourceId::parse("minecraft:ancient_cities")?;
        let rules = StructureBiomes::load(&mut provider, [structure.clone()])?;
        let mut cache = StructureSetCache::default();
        let placement = cache.load(&mut provider, &set)?.placement.clone();
        let chunk = placement.potential_chunk(12345, [0, 0]);
        let result = generator.try_generate_jigsaw_structure_set(
            &mut provider,
            &mut DensityEvaluator::default(),
            &mut cache,
            &rules,
            &set,
            chunk,
            |_| Ok(false),
            |value| {
                let name = value
                    .as_str()
                    .or_else(|| value.get("id").and_then(Value::as_str))
                    .context("invalid block")?;
                Ok(BlockHeightFlags {
                    is_air: name == "minecraft:air",
                    blocks_motion: name == "minecraft:stone",
                    blocks_motion_no_leaves: name == "minecraft:stone",
                    has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                })
            },
        )?;
        let StructureSetStart::Generated {
            structure: actual,
            start,
        } = result
        else {
            anyhow::bail!("jigsaw candidate did not generate");
        };
        assert_eq!(actual, structure);
        assert_eq!(start.chunk, chunk);
        assert!(!start.pieces.is_empty());
        assert_eq!(start.terrain_pieces()?.len(), start.pieces.len());
        let complete_blocks = provider.inner.vanilla_block_ids()?;
        let generated_blocks = provider.inner.vanilla_generated_block_registry()?;
        let prepared_processors =
            start.prepare_processors(&mut provider, &complete_blocks, &generated_blocks)?;
        assert_eq!(prepared_processors.len(), start.pieces.len());

        use super::super::{
            schedule::GenerationStage,
            structure_batch::{StructureGenerationBatch, StructurePreparationPlan},
        };
        use native_world::loaded::ChunkPos;
        let target = ChunkPos {
            x: chunk[0],
            z: chunk[1],
        };
        let mut batch = StructureGenerationBatch::new(generator, rules, vec![set])?;
        batch.request_references(target)?;
        while batch
            .step(&mut provider, &mut |value| {
                let name = value
                    .as_str()
                    .or_else(|| value.get("id").and_then(Value::as_str))
                    .context("invalid block")?;
                Ok(BlockHeightFlags {
                    is_air: name == "minecraft:air",
                    blocks_motion: name == "minecraft:stone",
                    blocks_motion_no_leaves: name == "minecraft:stone",
                    has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                })
            })?
            .is_some()
        {}
        assert!(batch.is_complete());
        assert!(batch.has_starts(target));
        assert!(batch.starts(target).is_none());
        let retained = batch
            .jigsaw_starts(target)
            .context("missing batch jigsaw")?
            .get(&structure)
            .context("missing city")?;
        assert_eq!(retained.to_tag(&structure)?, start.to_tag(&structure)?);
        let references = batch
            .references(target)
            .context("missing references")?
            .get(&structure)
            .context("missing city reference")?;
        let packed = ((chunk[1] as u32 as u64) << 32 | chunk[0] as u32 as u64) as i64;
        assert!(references.contains(&packed));
        let order = batch
            .unambiguous_terrain_start_order(target)?
            .context("unexpected ambiguous order")?;
        assert!(order.contains(&(structure.clone(), target)));
        let mut pieces = Vec::new();
        for (id, source) in &order {
            pieces.extend(
                batch
                    .jigsaw_starts(*source)
                    .context("missing referenced jigsaw")?[id]
                    .terrain_pieces()?,
            );
        }
        let expected = native_core::beardifier::Beardifier::for_chunk(chunk, &pieces)
            .map_err(anyhow::Error::msg)?;
        let actual = batch.beardifier_for_chunk(target, &order)?;
        assert_eq!(actual.affected_bounds(), expected.affected_bounds());
        for y in [-48, -27, 0, 40] {
            for x in [chunk[0] * 16, chunk[0] * 16 + 8] {
                for z in [chunk[1] * 16, chunk[1] * 16 + 8] {
                    assert_eq!(
                        actual.sample(x, y, z).to_bits(),
                        expected.sample(x, y, z).to_bits()
                    );
                }
            }
        }
        let schedule = native_resources::structure_decoration::StructureDecorationSchedule::load(
            &mut provider,
        )?;
        assert!(
            format!(
                "{:#}",
                batch
                    .mixed_decoration_plan(target, &schedule)
                    .err()
                    .context("jigsaw decoration must be rejected")?
            )
            .contains("jigsaw structure decoration")
        );
        let aware = batch.mixed_decoration_plan_with_jigsaw(target, &schedule)?;
        assert!(aware.iter().any(|entry| entry.entry.id == structure
            && entry.starts.iter().any(|start| matches!(
                start,
                super::super::structure_batch::PlannedStructureStart::Jigsaw(_)
            ))));
        let plan = StructurePreparationPlan::new([(target, GenerationStage::StructureReferences)])?;
        let mut stages = batch.prepare_additional_stages(&plan, None)?;
        let transferred = stages
            .jigsaw_starts
            .get(&target)
            .context("jigsaw was not transferred")?
            .get(&structure)
            .context("city missing")?;
        assert_eq!(transferred.to_tag(&structure)?, start.to_tag(&structure)?);
        assert!(
            format!(
                "{:#}",
                stages
                    .mixed_decoration_plan(target, &schedule)
                    .err()
                    .context("jigsaw decoration must be rejected")?
            )
            .contains("jigsaw structure decoration")
        );
        {
            let aware = stages.mixed_decoration_plan_with_jigsaw(target, &schedule)?;
            let keys = stages.mixed_commit_keys(&aware, false)?;
            assert!(
                keys.iter()
                    .flatten()
                    .any(|key| key == &(structure.clone(), target))
            );
        }
        let refs = stages
            .references
            .get(&target)
            .context("missing staged references")?;
        let sources = super::super::structure_data::referenced_start_chunks_with_jigsaw(
            target,
            &stages.starts,
            &stages.ocean_starts,
            &stages.mineshafts,
            &stages.jigsaw_starts,
            refs,
            &stages.huts,
        )?;
        assert!(sources.contains(&target));
        let data = super::super::structure_data::pack_structure_data_with_jigsaw(
            target,
            &stages.starts[&target],
            stages
                .ocean_starts
                .get(&target)
                .unwrap_or(&Default::default()),
            stages
                .mineshafts
                .get(&target)
                .unwrap_or(&Default::default()),
            &stages.jigsaw_starts[&target],
            refs,
            &stages.huts,
        )?;
        assert_eq!(
            data.get("starts")
                .and_then(|tag| tag.get("minecraft:ancient_city")),
            Some(&start.to_tag(&structure)?)
        );
        // An unsupported inline holder must abort the entire draft.
        let original = stages.jigsaw_starts[&target][&structure].clone();
        let first = &mut stages
            .jigsaw_starts
            .get_mut(&target)
            .unwrap()
            .get_mut(&structure)
            .unwrap()
            .pieces[0]
            .element;
        if let PoolElement::Single { processors, .. } = first {
            *processors = native_resources::structure_pool::HolderValue::Inline(
                serde_json::json!({"processors":[]}),
            );
        } else {
            anyhow::bail!("expected a single city center");
        }
        assert!(
            super::super::structure_data::pack_structure_data_with_jigsaw(
                target,
                &stages.starts[&target],
                &Default::default(),
                &Default::default(),
                &stages.jigsaw_starts[&target],
                refs,
                &stages.huts
            )
            .is_err()
        );
        stages
            .jigsaw_starts
            .get_mut(&target)
            .unwrap()
            .insert(structure.clone(), original);
        let registry: std::collections::BTreeSet<_> = provider
            .inner
            .vanilla_generated_block_registry()?
            .ids()
            .map(str::to_owned)
            .collect();
        let tags = native_resources::carver::tags::CarverBlockTags::load(&mut provider, &registry)?;
        let moved = super::super::FreshTerrainBatch::new_with_prepared_structures(
            batch,
            std::collections::BTreeMap::from([(target, order)]),
            tags,
            "minecraft:overworld",
        )?;
        assert!(moved.structure_starts(target).is_none());
        assert_eq!(
            moved
                .jigsaw_starts(target)
                .context("jigsaw missing from terrain batch")?[&structure]
                .to_tag(&structure)?,
            start.to_tag(&structure)?
        );
        assert!(
            moved.structure_data(target).is_err(),
            "unfinished features must not expose save data"
        );
        Ok(())
    }
}
