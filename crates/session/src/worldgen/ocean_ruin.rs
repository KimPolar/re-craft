//! Prepare ocean-ruin template selections without placing blocks or saving.
use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use native_persistence::nbt::Tag;
use native_persistence::structure_template::StructureTemplate;
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    ocean_ruin::{self, Piece, Settings, Temperature},
    packs::{ResourceId, ResourceProvider},
};
use serde_json::Value;

mod drowned_context;
mod placement;
pub use drowned_context::{
    DrownedGenerationContext, DrownedSpawnEffects, GenerationDifficultyInputs,
};
pub use placement::OceanRuinPlacementWorld;

#[derive(Clone)]
pub struct PreparedPiece {
    pub selection: Piece,
    pub template: Arc<StructureTemplate>,
    /// Java template bounds, updated when the piece is aligned to the floor.
    pub bounds: native_core::beardifier::BoundingBox,
}

/// A complete ocean-ruin start before any of its pieces are floor-aligned.
/// Piece order is significant during decoration and is kept as generated.
#[derive(Clone)]
pub struct PreparedStart {
    pub chunk: [i32; 2],
    pub generation_point: [i32; 3],
    pub terrain_adjustment: native_core::beardifier::TerrainAdjustment,
    pub pieces: Vec<PreparedPiece>,
}

impl PreparedStart {
    /// Attempt one start after the structure-set candidate and existing-start
    /// checks. Rejected biomes do not load structure templates.
    pub fn generate(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        environment: &mut impl native_resources::structure_start::StartEnvironment,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<Option<Self>> {
        use native_resources::structure_start::{
            Heightmap, center_generation_point, decode_terrain_adjustment,
        };
        Settings::decode(definition)?;
        let terrain_adjustment = decode_terrain_adjustment(definition)?;
        let Some(generation_point) =
            center_generation_point(environment, chunk, Heightmap::OceanFloor)?
        else {
            return Ok(None);
        };
        let pieces = prepare_pieces(provider, definition, seed, chunk)?;
        ensure!(!pieces.is_empty(), "ocean ruin produced no pieces");
        Ok(Some(Self {
            chunk,
            generation_point,
            terrain_adjustment,
            pieces,
        }))
    }

    /// The structure-reference stage uses the union of every initial piece.
    pub fn reference_bounds(
        &self,
        structure: ResourceId,
    ) -> Result<super::structure_references::StructureStartBounds> {
        let pieces: Vec<_> = self.pieces.iter().map(|piece| piece.bounds).collect();
        super::structure_references::StructureStartBounds::from_pieces(
            structure,
            &pieces,
            self.terrain_adjustment,
        )
    }
}

/// A structure block is retained for the later data-marker pass and must not
/// be placed as an ordinary template block.
#[derive(Clone, Debug, PartialEq)]
pub enum PreparedPaletteState {
    Block(native_world::loaded::BlockState),
    StructureMarker {
        properties: BTreeMap<String, String>,
    },
}

/// A surviving block after the ruin's per-block processors, before the
/// archaeology processor, chunk clipping, and actual placement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FirstPassBlock {
    pub source_index: usize,
    pub position: [i32; 3],
}

/// The replacement produced by the ruin's capped archaeology processor.
#[derive(Clone, Debug, PartialEq)]
pub struct ArchaeologyReplacement {
    pub state: native_world::loaded::BlockState,
    /// Template block-entity data with the loot fields applied.
    pub nbt: Tag,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcessedRuinBlock {
    pub original: FirstPassBlock,
    pub archaeology: Option<ArchaeologyReplacement>,
}

/// A structure-block DATA marker, retained separately from normal blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct RuinDataMarker {
    pub source_index: usize,
    pub position: [i32; 3],
    pub metadata: native_persistence::nbt::JavaString,
}

/// World-dependent parts of structure drowned initialization. Implementations
/// must isolate their state when cloned; the enclosing decoration attempt must
/// also stage the context until the whole region is published.
pub trait DrownedMarkerContext: Clone {
    /// Use the active base difficulty, overworld clock and position-dependent
    /// moon brightness. WorldGenRegion supplies zero inhabited time; see
    /// `LocalDifficulty::for_generation`.
    fn special_multiplier(
        &mut self,
        position: [i32; 3],
        world: &native_world::loaded::World,
    ) -> Result<f32>;
    fn halloween(&mut self) -> Result<bool>;
    fn try_existing_chicken(
        &mut self,
        position: [i32; 3],
        world: &native_world::loaded::World,
    ) -> Result<bool>;
    fn spawn_chicken_jockey(
        &mut self,
        position: [i32; 3],
        world: &native_world::loaded::World,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
    ) -> Result<bool>;
    /// Return the final item-stack NBT after the active enchantment provider.
    fn enchant_weapon(
        &mut self,
        position: [i32; 3],
        world: &native_world::loaded::World,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        weapon: native_resources::drowned_spawn::MainHandRoll,
        special: f32,
    ) -> Result<Tag>;
    fn more_frequent_drowned_biome(
        &mut self,
        position: [i32; 3],
        world: &native_world::loaded::World,
    ) -> Result<bool>;
    fn spawn_zombie_nautilus(
        &mut self,
        position: [i32; 3],
        world: &native_world::loaded::World,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        persistent: bool,
    ) -> Result<bool>;
}

impl PreparedPiece {
    /// Java creates a fresh position-seeded random source each time it chooses
    /// a palette, including for the later marker pass.
    pub fn selected_palette_index(&self) -> Result<Option<usize>> {
        let count = i32::try_from(self.template.palettes.len()).context("too many palettes")?;
        if count == 0 {
            return Ok(None);
        }
        let [x, y, z] = self.selection.position;
        let seed = native_resources::blockstates::positional_seed(x, y, z);
        Ok(Some(
            native_core::random::LegacyRandom::new(seed)
                .next_int_bound(count)
                .unwrap() as usize,
        ))
    }

    /// Find DATA markers from the unprocessed template after successful block
    /// placement. Decay does not remove them; only the placement box clips them.
    pub fn data_markers(
        &self,
        registry: &GeneratedBlockRegistry,
        clip: native_core::beardifier::BoundingBox,
    ) -> Result<Vec<RuinDataMarker>> {
        let Some(index) = self.selected_palette_index()? else {
            return Ok(Vec::new());
        };
        let palette = self.transformed_palette(registry, index)?;
        self.data_markers_with_palette(index, &palette, clip)
    }

    fn data_markers_with_palette(
        &self,
        palette_index: usize,
        palette: &[PreparedPaletteState],
        clip: native_core::beardifier::BoundingBox,
    ) -> Result<Vec<RuinDataMarker>> {
        let mut markers = Vec::new();
        for source_index in self.order_for_palette(palette_index, palette)? {
            let block = &self.template.blocks[source_index];
            if !matches!(
                &palette[block.state],
                PreparedPaletteState::StructureMarker { .. }
            ) {
                continue;
            }
            let position = self.block_position(block);
            if !(0..3).all(|i| (clip.min[i]..=clip.max[i]).contains(&position[i])) {
                continue;
            }
            let Some(nbt) = &block.nbt else { continue };
            let Some(Tag::String(mode)) = nbt.get("mode") else {
                anyhow::bail!("structure marker mode missing or invalid")
            };
            if mode.matches("SAVE") || mode.matches("LOAD") || mode.matches("CORNER") {
                continue;
            }
            ensure!(mode.matches("DATA"), "invalid structure marker mode");
            let metadata = match nbt.get("metadata") {
                Some(Tag::String(value)) => value.clone(),
                _ => "".into(),
            };
            markers.push(RuinDataMarker {
                source_index,
                position,
                metadata,
            });
        }
        Ok(markers)
    }

    /// Update the template origin and bounds as Java does before placement.
    pub fn align_to_floor(&mut self, world: &mut impl ocean_ruin::FloorWorld) -> Result<()> {
        let position = ocean_ruin::floor_position(&self.selection, self.template.size, world)?;
        let bounds = native_resources::structure_transform::bounding_box(
            self.template.size,
            position,
            native_resources::structure_transform::Mirror::None,
            self.selection.rotation,
            [0; 3],
        )?;
        self.selection.position = position;
        self.bounds = bounds;
        Ok(())
    }

    pub fn block_position(
        &self,
        block: &native_persistence::structure_template::TemplateBlock,
    ) -> [i32; 3] {
        native_resources::structure_transform::block_world_position(
            block.position,
            self.selection.position,
            native_resources::structure_transform::Mirror::None,
            self.selection.rotation,
            [0; 3],
        )
    }

    /// Transform one explicitly selected palette. The block list still uses
    /// NBT order; Java's placement ordering and processors remain separate.
    pub fn transformed_palette(
        &self,
        registry: &GeneratedBlockRegistry,
        palette_index: usize,
    ) -> Result<Vec<PreparedPaletteState>> {
        let palette = self
            .template
            .palettes
            .get(palette_index)
            .context("template palette index out of bounds")?;
        palette
            .iter()
            .map(|tag| {
                let name = match tag.get("id") {
                    Some(Tag::String(name)) => {
                        String::from_utf16(&name.0).context("invalid template block identifier")?
                    }
                    _ => anyhow::bail!("template block identifier missing"),
                };
                let mut properties = BTreeMap::new();
                if let Some(value) = tag.get("properties") {
                    let Tag::Compound(entries) = value else {
                        anyhow::bail!("template properties are not a compound")
                    };
                    for (key, value) in entries {
                        let key =
                            String::from_utf16(&key.0).context("invalid template property name")?;
                        let Tag::String(value) = value else {
                            anyhow::bail!("template property is not a string")
                        };
                        let value = String::from_utf16(&value.0)
                            .context("invalid template property value")?;
                        ensure!(
                            properties.insert(key, value).is_none(),
                            "duplicate template property"
                        );
                    }
                }
                let id = ResourceId::parse(&name)?;
                let name = format!("{}:{}", id.namespace, id.path);
                if name == "minecraft:structure_block" {
                    return Ok(PreparedPaletteState::StructureMarker { properties });
                }
                let input = serde_json::json!({"id": name, "properties": properties});
                let (name, properties) = registry.resolve(&input)?;
                let properties = registry.transform_template_state(
                    &name,
                    &properties,
                    native_resources::structure_transform::Mirror::None,
                    self.selection.rotation,
                )?;
                Ok(PreparedPaletteState::Block(
                    native_world::loaded::BlockState { name, properties },
                ))
            })
            .collect()
    }

    /// Calculate Java's local template load order. This includes structure
    /// blocks; later processors must still filter and handle their markers.
    pub fn block_order(
        &self,
        registry: &GeneratedBlockRegistry,
        palette_index: usize,
    ) -> Result<Vec<usize>> {
        // The supported ocean-ruin states have rotation-invariant static-full
        // classification, verified for all four template rotations.
        let palette = self.transformed_palette(registry, palette_index)?;
        self.order_for_palette(palette_index, &palette)
    }

    fn order_for_palette(
        &self,
        palette_index: usize,
        palette: &[PreparedPaletteState],
    ) -> Result<Vec<usize>> {
        self.template.java_block_order(palette_index, |index, _| {
            let PreparedPaletteState::Block(state) = &palette[index] else {
                anyhow::bail!("structure block without NBT needs collision classification")
            };
            ocean_template_static_full(state)
        })
    }

    /// Run Java's rot and air/structure-block ignore processors in load
    /// order over the entire piece. The selection position must already be
    /// adjusted to the ocean floor. Markers remain in the original template
    /// for their separate post-placement pass.
    pub fn first_pass_blocks(
        &self,
        registry: &GeneratedBlockRegistry,
        palette_index: usize,
    ) -> Result<Vec<FirstPassBlock>> {
        let palette = self.transformed_palette(registry, palette_index)?;
        self.first_pass_with_palette(palette_index, &palette)
    }

    fn first_pass_with_palette(
        &self,
        palette_index: usize,
        palette: &[PreparedPaletteState],
    ) -> Result<Vec<FirstPassBlock>> {
        use native_core::random::LegacyRandom;
        use native_resources::blockstates::positional_seed;
        ensure!(
            self.selection.integrity.is_finite() && (0.0..=1.0).contains(&self.selection.integrity),
            "invalid ocean ruin integrity"
        );
        let order = self.order_for_palette(palette_index, palette)?;
        let mut surviving = Vec::with_capacity(order.len());
        for source_index in order {
            let block = &self.template.blocks[source_index];
            let position = self.block_position(block);
            let mut random =
                LegacyRandom::new(positional_seed(position[0], position[1], position[2]));
            if random.next_float() > self.selection.integrity {
                continue;
            }
            match &palette[block.state] {
                PreparedPaletteState::StructureMarker { .. } => continue,
                PreparedPaletteState::Block(state) if state.name == "minecraft:air" => continue,
                PreparedPaletteState::Block(_) => {}
            }
            surviving.push(FirstPassBlock {
                source_index,
                position,
            });
        }
        Ok(surviving)
    }

    /// Apply the ruin's capped archaeology rule to the complete piece.
    /// This does not clip or place blocks, nor process structure markers.
    pub fn process_blocks(
        &self,
        registry: &GeneratedBlockRegistry,
        palette_index: usize,
        world_seed: i64,
        temperature: Temperature,
    ) -> Result<Vec<ProcessedRuinBlock>> {
        let palette = self.transformed_palette(registry, palette_index)?;
        self.process_blocks_with_palette(palette_index, &palette, world_seed, temperature)
    }

    fn process_blocks_with_palette(
        &self,
        palette_index: usize,
        palette: &[PreparedPaletteState],
        world_seed: i64,
        temperature: Temperature,
    ) -> Result<Vec<ProcessedRuinBlock>> {
        use native_core::{
            normal_noise::{NoiseSeedFactory, NoiseSeedSource},
            random::LegacyRandom,
        };
        use native_resources::blockstates::positional_seed;

        let mut blocks: Vec<_> = self
            .first_pass_with_palette(palette_index, palette)?
            .into_iter()
            .map(|original| ProcessedRuinBlock {
                original,
                archaeology: None,
            })
            .collect();
        if blocks.is_empty() {
            return Ok(blocks);
        }
        let len = i32::try_from(blocks.len()).context("too many ruin blocks")?;
        let [x, y, z] = self.selection.position;
        let mut random = LegacyRandom::new(world_seed)
            .fork_noise_factory()
            .at(x, y, z);
        let mut indices: Vec<usize> = (0..blocks.len()).collect();
        for end in (2..=len).rev() {
            let other = random.next_int_bound(end).expect("positive shuffle bound") as usize;
            indices.swap(end as usize - 1, other);
        }
        let (input, output, table) = match temperature {
            Temperature::Warm => (
                "minecraft:sand",
                "minecraft:suspicious_sand",
                "minecraft:archaeology/ocean_ruin_warm",
            ),
            Temperature::Cold => (
                "minecraft:gravel",
                "minecraft:suspicious_gravel",
                "minecraft:archaeology/ocean_ruin_cold",
            ),
        };
        let mut replaced = 0;
        for index in indices {
            let block = &mut blocks[index];
            let source = &self.template.blocks[block.original.source_index];
            let PreparedPaletteState::Block(state) = &palette[source.state] else {
                continue;
            };
            if state.name != input {
                continue;
            }
            let [x, y, z] = block.original.position;
            let seed = LegacyRandom::new(positional_seed(x, y, z)).next_long();
            let mut entries = match &source.nbt {
                Some(Tag::Compound(entries)) => entries.clone(),
                Some(_) => anyhow::bail!("invalid template block entity NBT"),
                None => Vec::new(),
            };
            entries.retain(|(key, _)| !key.matches("LootTable") && !key.matches("LootTableSeed"));
            entries.push(("LootTable".into(), Tag::String(table.into())));
            entries.push(("LootTableSeed".into(), Tag::Long(seed)));
            block.archaeology = Some(ArchaeologyReplacement {
                state: native_world::loaded::BlockState {
                    name: output.into(),
                    properties: BTreeMap::from([("dusted".into(), "0".into())]),
                },
                nbt: Tag::Compound(entries),
            });
            replaced += 1;
            if replaced == 5 {
                break;
            }
        }
        Ok(blocks)
    }
}

/// Verified collision classes for the vanilla ocean-ruin palette. Unknown
/// data-pack blocks remain errors until their dynamic shape is established.
pub(super) fn ocean_template_static_full(state: &native_world::loaded::BlockState) -> Result<bool> {
    let id = state
        .name
        .strip_prefix("minecraft:")
        .context("unknown template collision")?;
    if matches!(id, "air" | "cave_air" | "void_air") || id.ends_with("_stairs") {
        return Ok(false);
    }
    if matches!(
        id,
        "bricks"
            | "chiseled_sandstone"
            | "chiseled_stone_bricks"
            | "cobblestone"
            | "cracked_stone_bricks"
            | "cut_sandstone"
            | "dark_oak_planks"
            | "gravel"
            | "light_blue_terracotta"
            | "magma_block"
            | "mossy_cobblestone"
            | "mossy_stone_bricks"
            | "obsidian"
            | "polished_diorite"
            | "polished_granite"
            | "prismarine"
            | "sand"
            | "sandstone"
            | "sea_lantern"
            | "spruce_planks"
            | "stone_bricks"
    ) || id.ends_with("_glazed_terracotta")
    {
        return Ok(true);
    }
    anyhow::bail!(
        "template collision classification is unsupported for {}",
        state.name
    )
}

/// Read the exact selected templates from the same active data-pack stack.
/// Data fixing, processors, entity handling and actual placement remain open.
pub fn prepare_pieces(
    provider: &mut impl ResourceProvider,
    definition: &Value,
    seed: i64,
    chunk: [i32; 2],
) -> Result<Vec<PreparedPiece>> {
    let settings = Settings::decode(definition)?;
    let selected = ocean_ruin::select_for_chunk(seed, chunk, settings)?;
    let mut templates = BTreeMap::<String, Arc<StructureTemplate>>::new();
    let mut result = Vec::with_capacity(selected.len());
    for selection in selected {
        let template = if let Some(template) = templates.get(&selection.template) {
            Arc::clone(template)
        } else {
            let id = ResourceId::parse(&selection.template)?;
            let template = StructureTemplate::load(provider, &id)?
                .with_context(|| format!("missing ocean ruin template {}", selection.template))?;
            ensure!(
                template.data_version == Some(5023),
                "ocean ruin template {} needs data-version conversion",
                selection.template
            );
            let template = Arc::new(template);
            templates.insert(selection.template.clone(), Arc::clone(&template));
            template
        };
        let bounds = native_resources::structure_transform::bounding_box(
            template.size,
            selection.position,
            native_resources::structure_transform::Mirror::None,
            selection.rotation,
            [0; 3],
        )?;
        result.push(PreparedPiece {
            selection,
            template,
            bounds,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use native_persistence::nbt::{self, NamedTag, Tag, TagType};
    use serde_json::json;
    use std::io::Write;

    struct Data {
        bytes: Option<Vec<u8>>,
        reads: BTreeMap<String, usize>,
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            *self.reads.entry(path.to_owned()).or_default() += 1;
            Ok(self.bytes.clone())
        }
    }
    fn template(version: i32) -> Vec<u8> {
        let list = |kind, values| Tag::List {
            element_type: kind,
            values,
        };
        let doc = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(version)),
                (
                    "size".into(),
                    list(TagType::Int, vec![Tag::Int(1), Tag::Int(1), Tag::Int(1)]),
                ),
                (
                    "palette".into(),
                    list(TagType::Compound, vec![Tag::Compound(vec![])]),
                ),
                ("blocks".into(), list(TagType::Compound, vec![])),
                ("entities".into(), list(TagType::Compound, vec![])),
            ]),
        };
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(&doc).unwrap()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn prepares_active_templates_once_and_rejects_missing_or_old_data() {
        let definition = json!({"type":"minecraft:ocean_ruin","biome_temp":"cold","large_probability":1.0,"cluster_probability":1.0});
        let mut data = Data {
            bytes: Some(template(5023)),
            reads: BTreeMap::new(),
        };
        let prepared = prepare_pieces(&mut data, &definition, 42, [0, 0]).unwrap();
        assert!(!prepared.is_empty());
        assert!(data.reads.values().all(|&count| count == 1));
        for (index, piece) in prepared.iter().enumerate() {
            for other in &prepared[..index] {
                if other.selection.template == piece.selection.template {
                    assert!(Arc::ptr_eq(&other.template, &piece.template));
                }
            }
        }
        data.bytes = Some(template(4000));
        assert!(
            format!(
                "{:#}",
                prepare_pieces(&mut data, &definition, 42, [0, 0])
                    .err()
                    .unwrap()
            )
            .contains("data-version conversion")
        );
        data.bytes = None;
        assert!(
            format!(
                "{:#}",
                prepare_pieces(&mut data, &definition, 42, [0, 0])
                    .err()
                    .unwrap()
            )
            .contains("missing ocean ruin template")
        );
    }

    #[test]
    fn start_checks_biomes_before_loading_and_references_every_piece() -> Result<()> {
        use native_core::beardifier::TerrainAdjustment;
        use native_resources::structure_start::{BiomeQuery, Heightmap, StartEnvironment};

        struct Environment {
            column: bool,
            point: bool,
            heights: usize,
        }
        impl StartEnvironment for Environment {
            fn vertical_bounds(&self) -> (i32, i32) {
                (-64, 319)
            }
            fn sea_level(&self) -> i32 {
                63
            }
            fn first_occupied_height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32> {
                assert_eq!((x, z, map), (-40, 40, Heightmap::OceanFloor));
                self.heights += 1;
                Ok(42)
            }
            fn biome_allowed(&mut self, _: [i32; 3], query: BiomeQuery) -> Result<bool> {
                Ok(match query {
                    BiomeQuery::Column => self.column,
                    BiomeQuery::Point => self.point,
                })
            }
        }
        let definition = json!({"type":"minecraft:ocean_ruin","biome_temp":"cold","large_probability":1.0,"cluster_probability":1.0,"terrain_adaptation":"beard_thin"});
        let mut data = Data {
            bytes: Some(template(5023)),
            reads: BTreeMap::new(),
        };
        let mut env = Environment {
            column: false,
            point: false,
            heights: 0,
        };
        assert!(PreparedStart::generate(&mut data, &definition, &mut env, 42, [-3, 2])?.is_none());
        assert_eq!(env.heights, 0);
        assert!(data.reads.is_empty());
        env.column = true;
        assert!(PreparedStart::generate(&mut data, &definition, &mut env, 42, [-3, 2])?.is_none());
        assert_eq!(env.heights, 1);
        assert!(data.reads.is_empty());
        env.point = true;
        let start =
            PreparedStart::generate(&mut data, &definition, &mut env, 42, [-3, 2])?.unwrap();
        assert_eq!(start.generation_point, [-40, 42, 40]);
        assert_eq!(start.chunk, [-3, 2]);
        assert_eq!(start.terrain_adjustment, TerrainAdjustment::BeardThin);
        assert!(start.pieces.len() > 1);
        let bounds = start
            .reference_bounds(ResourceId::parse("test:ruin")?)?
            .bounds;
        for piece in &start.pieces {
            for axis in 0..3 {
                assert!(bounds.min[axis] <= piece.bounds.min[axis]);
                assert!(bounds.max[axis] >= piece.bounds.max[axis]);
            }
        }
        assert_eq!(data.reads.values().copied().max(), Some(1));
        data.bytes = None;
        assert!(PreparedStart::generate(&mut data, &definition, &mut env, 42, [-3, 2]).is_err());
        Ok(())
    }

    #[test]
    fn first_pass_preserves_order_and_ignores_only_air_and_markers() {
        use native_persistence::structure_template::TemplateBlock;
        use native_resources::structure_transform::Rotation;
        use native_world::loaded::BlockState;

        let state = |name: &str| {
            PreparedPaletteState::Block(BlockState {
                name: format!("minecraft:{name}"),
                properties: BTreeMap::new(),
            })
        };
        let palette = vec![
            state("stone_bricks"),
            state("air"),
            state("cave_air"),
            PreparedPaletteState::StructureMarker {
                properties: BTreeMap::new(),
            },
        ];
        let blocks = vec![
            TemplateBlock {
                position: [2, 0, 0],
                state: 0,
                nbt: None,
            },
            TemplateBlock {
                position: [0, 0, 0],
                state: 1,
                nbt: None,
            },
            TemplateBlock {
                position: [1, 0, 0],
                state: 2,
                nbt: None,
            },
            TemplateBlock {
                position: [0, 1, 0],
                state: 3,
                nbt: Some(Tag::Compound(vec![
                    ("mode".into(), Tag::String("DATA".into())),
                    ("metadata".into(), Tag::String("chest".into())),
                ])),
            },
        ];
        let mut piece = PreparedPiece {
            selection: Piece {
                template: "minecraft:test".into(),
                position: [10, 40, -5],
                rotation: Rotation::Clockwise90,
                integrity: 1.0,
                large: false,
            },
            template: Arc::new(StructureTemplate {
                document: NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![]),
                },
                data_version: Some(5023),
                size: [3, 2, 1],
                palettes: vec![vec![Tag::Compound(vec![]); 4]],
                blocks,
                entities: vec![],
            }),
            bounds: native_core::beardifier::BoundingBox {
                min: [0; 3],
                max: [0; 3],
            },
        };
        assert_eq!(
            piece.first_pass_with_palette(0, &palette).unwrap(),
            vec![
                FirstPassBlock {
                    source_index: 0,
                    position: [10, 40, -3]
                },
                FirstPassBlock {
                    source_index: 2,
                    position: [10, 40, -4]
                },
            ]
        );
        piece.selection.integrity = 0.0;
        let clip = native_core::beardifier::BoundingBox::new([10, 41, -5], [10, 41, -5]).unwrap();
        let markers = piece.data_markers_with_palette(0, &palette, clip).unwrap();
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].source_index, 3);
        assert_eq!(markers[0].position, [10, 41, -5]);
        assert!(markers[0].metadata.matches("chest"));
        let outside = native_core::beardifier::BoundingBox::new([0; 3], [0; 3]).unwrap();
        assert!(
            piece
                .data_markers_with_palette(0, &palette, outside)
                .unwrap()
                .is_empty()
        );
        Arc::make_mut(&mut piece.template).blocks[3].nbt = Some(Tag::Compound(vec![(
            "mode".into(),
            Tag::String("invalid".into()),
        )]));
        assert!(piece.data_markers_with_palette(0, &palette, clip).is_err());
        assert!(
            piece
                .data_markers_with_palette(0, &palette, outside)
                .unwrap()
                .is_empty()
        );
        piece.selection.integrity = f32::NAN;
        assert!(piece.first_pass_with_palette(0, &palette).is_err());
    }

    #[test]
    fn archaeology_caps_replacements_and_preserves_other_nbt() {
        use native_persistence::structure_template::TemplateBlock;
        use native_resources::structure_transform::Rotation;
        use native_world::loaded::BlockState;
        let palette: Vec<_> = ["sand", "gravel", "stone_bricks"]
            .into_iter()
            .map(|name| {
                PreparedPaletteState::Block(BlockState {
                    name: format!("minecraft:{name}"),
                    properties: BTreeMap::new(),
                })
            })
            .collect();
        let blocks = (0..11)
            .map(|x| TemplateBlock {
                position: [x, 0, 0],
                state: if x < 8 {
                    0
                } else if x < 10 {
                    1
                } else {
                    2
                },
                nbt: Some(Tag::Compound(vec![
                    ("Keep".into(), Tag::Int(x)),
                    ("LootTable".into(), Tag::String("old".into())),
                    ("LootTableSeed".into(), Tag::Long(123)),
                ])),
            })
            .collect();
        let piece = PreparedPiece {
            selection: Piece {
                template: "minecraft:test".into(),
                position: [-20, 40, 9],
                rotation: Rotation::None,
                integrity: 1.0,
                large: false,
            },
            template: Arc::new(StructureTemplate {
                document: NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![]),
                },
                data_version: Some(5023),
                size: [11, 1, 1],
                palettes: vec![vec![Tag::Compound(vec![]); 3]],
                blocks,
                entities: vec![],
            }),
            bounds: native_core::beardifier::BoundingBox {
                min: [0; 3],
                max: [0; 3],
            },
        };
        let run = |temperature| {
            piece
                .process_blocks_with_palette(0, &palette, 42, temperature)
                .unwrap()
        };
        let warm = run(Temperature::Warm);
        assert_eq!(warm, run(Temperature::Warm));
        let replacements: Vec<_> = warm
            .iter()
            .filter_map(|block| {
                block
                    .archaeology
                    .as_ref()
                    .map(|replacement| (block, replacement))
            })
            .collect();
        assert_eq!(replacements.len(), 5);
        for (block, replacement) in replacements {
            assert_eq!(replacement.state.name, "minecraft:suspicious_sand");
            assert_eq!(
                replacement.nbt.get("Keep"),
                Some(&Tag::Int(block.original.source_index as i32))
            );
            assert!(
                matches!(replacement.nbt.get("LootTable"), Some(Tag::String(id)) if id.matches("minecraft:archaeology/ocean_ruin_warm"))
            );
            assert!(matches!(
                replacement.nbt.get("LootTableSeed"),
                Some(Tag::Long(_))
            ));
        }
        let cold = run(Temperature::Cold);
        let replacements: Vec<_> = cold
            .iter()
            .filter_map(|block| block.archaeology.as_ref())
            .collect();
        assert_eq!(replacements.len(), 2);
        assert!(
            replacements
                .iter()
                .all(|replacement| replacement.state.name == "minecraft:suspicious_gravel")
        );
        assert!(replacements.iter().all(|replacement| matches!(replacement.nbt.get("LootTable"), Some(Tag::String(id)) if id.matches("minecraft:archaeology/ocean_ruin_cold"))));
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn prepares_selected_vanilla_ocean_ruin_templates() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        let registry = data.vanilla_generated_block_registry()?;
        let mut markers = 0;
        let mut glazed = 0;
        for temperature in ["warm", "cold"] {
            let definition = json!({"type":"minecraft:ocean_ruin","biome_temp":temperature,"large_probability":0.5,"cluster_probability":0.9});
            for seed in 0..16 {
                let pieces = prepare_pieces(&mut data, &definition, seed, [-3, 2])?;
                assert!(!pieces.is_empty());
                assert!(pieces.iter().all(|piece| !piece.template.blocks.is_empty()));
                for piece in &pieces {
                    for index in 0..piece.template.palettes.len() {
                        let palette = piece.transformed_palette(&registry, index)?;
                        assert_eq!(palette.len(), piece.template.palettes[index].len());
                        for state in palette {
                            match state {
                                PreparedPaletteState::StructureMarker { .. } => markers += 1,
                                PreparedPaletteState::Block(state)
                                    if state.name.ends_with("_glazed_terracotta") =>
                                {
                                    glazed += 1
                                }
                                _ => {}
                            }
                        }
                    }
                    for block in &piece.template.blocks {
                        let pos = piece.block_position(block);
                        assert!((0..3).all(|axis| {
                            (piece.bounds.min[axis]..=piece.bounds.max[axis]).contains(&pos[axis])
                        }));
                    }
                }
            }
        }
        assert!(markers > 0);
        assert!(glazed > 0);
        Ok(())
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn vanilla_ruin_first_pass_matches_java() -> Result<()> {
        compare_vanilla_ruin_processors(false)
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn vanilla_ruin_archaeology_matches_java() -> Result<()> {
        compare_vanilla_ruin_processors(true)
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn vanilla_ruin_markers_and_palettes_match_java() -> Result<()> {
        use native_resources::structure_transform::Rotation;
        use std::{path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
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
        let classpath = std::env::join_paths(paths)?;
        let source = temp.path().join("OceanRuinMarkersOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/OceanRuinMarkersOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut names = Vec::new();
        for prefix in ["brick", "cracked", "mossy", "warm"] {
            for n in 1..=8 {
                names.push(format!("underwater_ruin/{prefix}_{n}"));
            }
        }
        for prefix in ["big_brick", "big_cracked", "big_mossy"] {
            for n in [1, 2, 3, 8] {
                names.push(format!("underwater_ruin/{prefix}_{n}"));
            }
        }
        for n in 4..=7 {
            names.push(format!("underwater_ruin/big_warm_{n}"));
        }
        let result = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("OceanRuinMarkersOracle")
            .args(&names)
            .output()?;
        ensure!(
            result.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let output = String::from_utf8(result.stdout)?;
        let mut rows = output.lines().filter(|line| {
            line.starts_with("P\t") || line.starts_with("T\t") || line.starts_with("M\t")
        });
        for count in [1, 2, 3, 7, 16, 257] {
            let template = Arc::new(StructureTemplate {
                document: NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![]),
                },
                data_version: Some(5023),
                size: [1; 3],
                palettes: vec![vec![]; count],
                blocks: vec![],
                entities: vec![],
            });
            let mut piece = PreparedPiece {
                selection: Piece {
                    template: "minecraft:test".into(),
                    position: [0; 3],
                    rotation: Rotation::None,
                    integrity: 1.0,
                    large: false,
                },
                template,
                bounds: native_core::beardifier::BoundingBox::new([0; 3], [0; 3]).unwrap(),
            };
            for i in 0..128 {
                piece.selection.position = [i * 173 - 10000, i * 19 % 511 - 255, 1000 - i * 237];
                let row = rows.next().context("missing palette row")?;
                let fields: Vec<_> = row.split('\t').collect();
                assert_eq!(fields[0], "P");
                assert_eq!(fields[1].parse::<usize>()?, count);
                assert_eq!(fields[2].parse::<i32>()?, i);
                assert_eq!(piece.selected_palette_index()?, Some(fields[3].parse()?));
            }
        }
        struct Flat(i32);
        impl ocean_ruin::FloorWorld for Flat {
            fn min_y(&self) -> i32 {
                -64
            }
            fn ocean_floor_height(&mut self, _: i32, _: i32) -> Result<i32> {
                Ok(self.0)
            }
            fn floor_scan_passable(&mut self, _: [i32; 3]) -> Result<bool> {
                Ok(false)
            }
        }
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let rotations = [
            Rotation::None,
            Rotation::Clockwise90,
            Rotation::Clockwise180,
            Rotation::Counterclockwise90,
        ];
        let mut total = 0;
        for (i, name) in names.iter().enumerate() {
            let row = rows.next().context("missing template header")?;
            let fields: Vec<_> = row.split('\t').collect();
            assert_eq!(fields[0], "T");
            assert_eq!(fields[1], name);
            let count: usize = fields[2].parse()?;
            let expected_min: [i32; 3] =
                [fields[3].parse()?, fields[4].parse()?, fields[5].parse()?];
            let expected_max: [i32; 3] =
                [fields[6].parse()?, fields[7].parse()?, fields[8].parse()?];
            let origin = [i as i32 * 7 - 120, 45 + i as i32 % 7, 170 - i as i32 * 13];
            let template = Arc::new(
                StructureTemplate::load(&mut resources, &ResourceId::parse(name)?)?
                    .context("missing template")?,
            );
            let mut piece = PreparedPiece {
                selection: Piece {
                    template: format!("minecraft:{name}"),
                    position: [origin[0], 90, origin[2]],
                    rotation: rotations[i % 4],
                    integrity: 0.0,
                    large: false,
                },
                template,
                bounds: native_core::beardifier::BoundingBox::new([0; 3], [0; 3]).unwrap(),
            };
            piece.align_to_floor(&mut Flat(origin[1]))?;
            assert_eq!(piece.bounds.min, expected_min, "{name}");
            assert_eq!(piece.bounds.max, expected_max, "{name}");
            let radius = if i % 2 == 0 { 32 } else { 3 };
            let clip = native_core::beardifier::BoundingBox::new(
                origin.map(|v| v - radius),
                origin.map(|v| v + radius),
            )
            .unwrap();
            let markers = piece.data_markers(&registry, clip)?;
            assert_eq!(markers.len(), count, "{name}");
            for marker in markers {
                let row = rows.next().context("missing marker")?;
                let fields: Vec<_> = row.split('\t').collect();
                assert_eq!(fields[0], "M");
                let position: [i32; 3] =
                    [fields[1].parse()?, fields[2].parse()?, fields[3].parse()?];
                assert_eq!(marker.position, position, "{name}");
                assert!(marker.metadata.matches(fields[4]), "{name}");
                total += 1;
            }
        }
        assert!(rows.next().is_none());
        assert!(total > 0);
        println!(
            "verified 768 palette choices and {total} clipped markers with updated bounds across {} ruins",
            names.len()
        );
        Ok(())
    }

    fn compare_vanilla_ruin_processors(archaeology: bool) -> Result<()> {
        use native_resources::structure_transform::{Mirror, Rotation};
        use std::{path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
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
        let classpath = std::env::join_paths(paths)?;
        let (class, code) = if archaeology {
            (
                "OceanRuinArchaeologyOracle",
                include_str!("../../tests/java/OceanRuinArchaeologyOracle.java"),
            )
        } else {
            (
                "OceanRuinFirstPassOracle",
                include_str!("../../tests/java/OceanRuinFirstPassOracle.java"),
            )
        };
        let source = temp.path().join(format!("{class}.java"));
        std::fs::write(&source, code)?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut names = Vec::new();
        for prefix in ["brick", "cracked", "mossy", "warm"] {
            for n in 1..=8 {
                names.push(format!("underwater_ruin/{prefix}_{n}"));
            }
        }
        for prefix in ["big_brick", "big_cracked", "big_mossy"] {
            for n in [1, 2, 3, 8] {
                names.push(format!("underwater_ruin/{prefix}_{n}"));
            }
        }
        for n in 4..=7 {
            names.push(format!("underwater_ruin/big_warm_{n}"));
        }
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg(class)
            .args(&names)
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout)?;
        let mut rows = output.lines().filter(|line| {
            line.starts_with("T\t") || line.starts_with("B\t") || line.starts_with("A\t")
        });
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let rotations = [
            Rotation::None,
            Rotation::Clockwise90,
            Rotation::Clockwise180,
            Rotation::Counterclockwise90,
        ];
        let integrities = [0.5, 0.7, 0.8, 0.9];
        let mut total = 0;
        let mut replaced = 0;
        for (i, name) in names.iter().enumerate() {
            let header = rows.next().context("missing Java template")?;
            let mut fields = header.split('\t');
            ensure!(
                fields.next() == Some("T") && fields.next() == Some(name.as_str()),
                "unexpected template {header}"
            );
            let count: usize = fields.next().context("missing count")?.parse()?;
            let position = [-101 + i as i32 * 7, 41 + i as i32 % 11, 207 - i as i32 * 13];
            let rotation = rotations[i % 4];
            let template = Arc::new(
                StructureTemplate::load(&mut resources, &ResourceId::parse(name)?)?
                    .context("missing template")?,
            );
            let bounds = native_resources::structure_transform::bounding_box(
                template.size,
                position,
                Mirror::None,
                rotation,
                [0; 3],
            )?;
            let piece = PreparedPiece {
                selection: Piece {
                    template: format!("minecraft:{name}"),
                    position,
                    rotation,
                    integrity: integrities[(i / 4) % 4],
                    large: false,
                },
                template,
                bounds,
            };
            let palette = piece.transformed_palette(&registry, 0)?;
            let blocks = if archaeology {
                let temperature = if name.contains("warm") {
                    Temperature::Warm
                } else {
                    Temperature::Cold
                };
                let seed = (i as i64 * 1_234_567_891_011) ^ -982_451_653;
                piece.process_blocks(&registry, 0, seed, temperature)?
            } else {
                piece
                    .first_pass_blocks(&registry, 0)?
                    .into_iter()
                    .map(|original| ProcessedRuinBlock {
                        original,
                        archaeology: None,
                    })
                    .collect()
            };
            assert_eq!(blocks.len(), count, "{name}");
            for block in blocks {
                let row = rows.next().context("missing Java block")?;
                let mut fields = row.split('\t');
                let kind = fields.next().context("missing row kind")?;
                let position: [i32; 3] = [
                    fields.next().context("x")?.parse()?,
                    fields.next().context("y")?.parse()?,
                    fields.next().context("z")?.parse()?,
                ];
                let id = fields.next().context("block ID")?;
                let original = &piece.template.blocks[block.original.source_index];
                let PreparedPaletteState::Block(state) = &palette[original.state] else {
                    anyhow::bail!("marker survived first pass")
                };
                assert_eq!(block.original.position, position, "{name}");
                if let Some(replacement) = block.archaeology {
                    assert_eq!(kind, "A", "{name}");
                    assert_eq!(replacement.state.name, id, "{name}");
                    let table = fields.next().context("loot table")?;
                    let seed: i64 = fields.next().context("loot seed")?.parse()?;
                    let dusted = fields.next().context("dusted")?;
                    assert!(
                        matches!(replacement.nbt.get("LootTable"), Some(Tag::String(value)) if value.matches(table)),
                        "{name}"
                    );
                    assert_eq!(
                        replacement.nbt.get("LootTableSeed"),
                        Some(&Tag::Long(seed)),
                        "{name}"
                    );
                    assert_eq!(
                        replacement
                            .state
                            .properties
                            .get("dusted")
                            .map(String::as_str),
                        Some(dusted),
                        "{name}"
                    );
                    replaced += 1;
                } else {
                    assert_eq!(kind, "B", "{name}");
                    let nbt: bool = fields.next().context("NBT flag")?.parse()?;
                    assert_eq!(state.name, id, "{name}");
                    assert_eq!(original.nbt.is_some(), nbt, "{name}");
                }
                total += 1;
            }
        }
        assert!(rows.next().is_none());
        println!(
            "verified {total} processed blocks and {replaced} archaeology replacements across {} ruins",
            names.len()
        );
        if archaeology {
            assert!(replaced > 0);
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn vanilla_ruin_template_order_matches_java() -> Result<()> {
        use native_resources::structure_transform::Rotation;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
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
        let classpath = std::env::join_paths(paths)?;
        let source = temp.path().join("OceanRuinTemplateOrderOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/OceanRuinTemplateOrderOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut names = Vec::new();
        for prefix in ["brick", "cracked", "mossy", "warm"] {
            for n in 1..=8 {
                names.push(format!("underwater_ruin/{prefix}_{n}"));
            }
        }
        for prefix in ["big_brick", "big_cracked", "big_mossy"] {
            for n in [1, 2, 3, 8] {
                names.push(format!("underwater_ruin/{prefix}_{n}"));
            }
        }
        for n in 4..=7 {
            names.push(format!("underwater_ruin/big_warm_{n}"));
        }
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("OceanRuinTemplateOrderOracle")
            .args(&names)
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout)?;
        let mut rows = output
            .lines()
            .filter(|line| line.starts_with("T\t") || line.starts_with("B\t"));
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let mut total = 0;
        for name in &names {
            let header = rows.next().context("missing Java template")?;
            let mut fields = header.split('\t');
            ensure!(
                fields.next() == Some("T") && fields.next() == Some(name.as_str()),
                "unexpected Java template {header}"
            );
            let expected_count: usize = fields.next().context("missing Java count")?.parse()?;
            let template = Arc::new(
                StructureTemplate::load(&mut resources, &ResourceId::parse(name)?)?
                    .context("missing template")?,
            );
            let bounds = native_resources::structure_transform::bounding_box(
                template.size,
                [0, 90, 0],
                native_resources::structure_transform::Mirror::None,
                Rotation::None,
                [0; 3],
            )?;
            let mut piece = PreparedPiece {
                selection: Piece {
                    template: format!("minecraft:{name}"),
                    position: [0, 90, 0],
                    rotation: Rotation::None,
                    integrity: 0.8,
                    large: false,
                },
                template,
                bounds,
            };
            let palette = piece.transformed_palette(&registry, 0)?;
            let order = piece.block_order(&registry, 0)?;
            for rotation in [
                Rotation::Clockwise90,
                Rotation::Clockwise180,
                Rotation::Counterclockwise90,
            ] {
                piece.selection.rotation = rotation;
                assert_eq!(
                    piece.block_order(&registry, 0)?,
                    order,
                    "rotation changed load order for {name}"
                );
            }
            piece.selection.rotation = Rotation::None;
            assert_eq!(order.len(), expected_count, "{name}");
            for index in order {
                let row: Value = serde_json::from_str(
                    rows.next()
                        .context("missing Java block")?
                        .strip_prefix("B\t")
                        .context("unexpected Java row")?,
                )?;
                let block = &piece.template.blocks[index];
                assert_eq!(
                    block.position,
                    [
                        row["x"].as_i64().unwrap() as i32,
                        row["y"].as_i64().unwrap() as i32,
                        row["z"].as_i64().unwrap() as i32
                    ],
                    "{name}"
                );
                assert_eq!(block.nbt.is_some(), row["nbt"].as_bool().unwrap(), "{name}");
                let (id, properties) = match &palette[block.state] {
                    PreparedPaletteState::Block(state) => (state.name.as_str(), &state.properties),
                    PreparedPaletteState::StructureMarker { properties } => {
                        ("minecraft:structure_block", properties)
                    }
                };
                assert_eq!(id, row["id"].as_str().unwrap(), "{name}");
                assert_eq!(
                    serde_json::to_value(properties)?,
                    row["properties"],
                    "{name}"
                );
                total += 1;
            }
        }
        assert!(rows.next().is_none());
        println!(
            "verified {total} ordered template blocks across {} ruins",
            names.len()
        );
        Ok(())
    }
}
