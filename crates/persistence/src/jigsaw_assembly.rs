//! Active data-pack templates and pools for the jigsaw graph assembler.
//!
//! This prepares geometry only. Template processors, feature placement,
//! structure saving and old data-version conversion remain separate stages.
use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use native_core::beardifier::BoundingBox;
use native_resources::{
    jigsaw::{Connector, Joint, Orientation},
    jigsaw_alias::PoolAliases,
    jigsaw_placement::AssemblyContext,
    packs::{ResourceId, ResourceProvider},
    structure_pool::{PoolElement, TemplatePool},
    structure_transform::{self, Mirror, Rotation},
};

use crate::structure_template::StructureTemplate;

/// Caches pool and template snapshots for one immutable active pack stack.
pub struct TemplateAssemblyContext<'a, P, H> {
    provider: &'a mut P,
    height: H,
    aliases: BTreeMap<ResourceId, ResourceId>,
    pools: BTreeMap<ResourceId, Option<Arc<TemplatePool>>>,
    templates: BTreeMap<ResourceId, Arc<StructureTemplate>>,
}

impl<'a, P: ResourceProvider, H> TemplateAssemblyContext<'a, P, H> {
    pub fn new(provider: &'a mut P, height: H, aliases: BTreeMap<ResourceId, ResourceId>) -> Self {
        Self {
            provider,
            height,
            aliases,
            pools: BTreeMap::new(),
            templates: BTreeMap::new(),
        }
    }

    /// Resolve the structure's aliases once, using the sampled start position
    /// and independent positional RNG, before any pool lookup or assembly.
    pub fn for_structure_start(
        provider: &'a mut P,
        height: H,
        aliases: &PoolAliases,
        world_seed: i64,
        position: [i32; 3],
    ) -> Result<Self> {
        Ok(Self::new(
            provider,
            height,
            aliases.resolve(world_seed, position)?,
        ))
    }

    fn template(&mut self, id: &ResourceId) -> Result<Arc<StructureTemplate>> {
        if let Some(template) = self.templates.get(id) {
            return Ok(Arc::clone(template));
        }
        let template = StructureTemplate::load(self.provider, id)?
            .with_context(|| format!("missing jigsaw template {}:{}", id.namespace, id.path))?;
        ensure!(
            template.data_version == Some(5023),
            "jigsaw template requires unsupported data-version conversion"
        );
        let template = Arc::new(template);
        self.templates.insert(id.clone(), Arc::clone(&template));
        Ok(template)
    }
}

impl<P, H> AssemblyContext for TemplateAssemblyContext<'_, P, H>
where
    P: ResourceProvider,
    H: FnMut([i32; 3]) -> Result<i32>,
{
    fn pool(&mut self, id: &ResourceId) -> Result<Option<Arc<TemplatePool>>> {
        if let Some(pool) = self.pools.get(id) {
            return Ok(pool.clone());
        }
        let pool = TemplatePool::load_optional(self.provider, id)?.map(Arc::new);
        self.pools.insert(id.clone(), pool.clone());
        Ok(pool)
    }

    fn connectors(
        &mut self,
        element: &PoolElement,
        origin: [i32; 3],
        rotation: Rotation,
    ) -> Result<Vec<Connector>> {
        match element {
            PoolElement::Single { location, .. } => {
                self.template(location)?.jigsaws_at(origin, rotation)
            }
            PoolElement::List { elements, .. } => self.connectors(
                elements.first().context("empty jigsaw element list")?,
                origin,
                rotation,
            ),
            PoolElement::Feature { .. } => {
                let empty = ResourceId::parse("minecraft:empty")?;
                Ok(vec![Connector {
                    position: origin,
                    orientation: Orientation::parse("down_south")?,
                    joint: Joint::Rollable,
                    name: None,
                    target: empty.clone(),
                    pool: empty,
                    placement_priority: 0,
                    selection_priority: 0,
                }])
            }
            PoolElement::Empty => Ok(Vec::new()),
        }
    }

    fn bounds(
        &mut self,
        element: &PoolElement,
        origin: [i32; 3],
        rotation: Rotation,
    ) -> Result<BoundingBox> {
        match element {
            PoolElement::Single { location, .. } => structure_transform::bounding_box(
                self.template(location)?.size,
                origin,
                Mirror::None,
                rotation,
                [0; 3],
            ),
            PoolElement::Feature { .. } => {
                BoundingBox::new(origin, origin).map_err(anyhow::Error::msg)
            }
            PoolElement::List { elements, .. } => {
                let mut combined: Option<BoundingBox> = None;
                for element in elements {
                    if matches!(element, PoolElement::Empty) {
                        continue;
                    }
                    let bounds = self.bounds(element, origin, rotation)?;
                    combined = Some(match combined {
                        None => bounds,
                        Some(previous) => BoundingBox::new(
                            std::array::from_fn(|i| previous.min[i].min(bounds.min[i])),
                            std::array::from_fn(|i| previous.max[i].max(bounds.max[i])),
                        )
                        .map_err(anyhow::Error::msg)?,
                    });
                }
                combined.context("jigsaw list has no bounding box")
            }
            PoolElement::Empty => bail!("empty jigsaw element has no bounding box"),
        }
    }

    fn surface_height(&mut self, position: [i32; 3]) -> Result<i32> {
        (self.height)(position)
    }

    fn resolve_alias(&mut self, id: &ResourceId) -> Result<ResourceId> {
        Ok(self.aliases.get(id).cloned().unwrap_or_else(|| id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::jigsaw_placement::{Piece, assemble};
    use std::collections::VecDeque;

    #[test]
    fn context_caches_missing_pools_and_models_feature_connector() -> Result<()> {
        use native_resources::{jigsaw::Joint, structure_pool::HolderValue};
        #[derive(Default)]
        struct EmptyProvider {
            reads: usize,
        }
        impl ResourceProvider for EmptyProvider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                self.reads += 1;
                Ok(None)
            }
        }
        let mut provider = EmptyProvider::default();
        let alias = ResourceId::parse("example:alias")?;
        let resolved = ResourceId::parse("example:real")?;
        {
            let mut context = TemplateAssemblyContext::new(
                &mut provider,
                |_| Ok(64),
                BTreeMap::from([(alias.clone(), resolved.clone())]),
            );
            assert_eq!(context.resolve_alias(&alias)?, resolved);
            assert!(context.pool(&resolved)?.is_none());
            assert!(context.pool(&resolved)?.is_none());
            let element = PoolElement::Feature {
                feature: HolderValue::Reference(ResourceId::parse("example:feature")?),
                projection: native_resources::structure_pool::Projection::Rigid,
            };
            let at = [3, 4, 5];
            let connectors = context.connectors(&element, at, Rotation::Clockwise90)?;
            assert_eq!(connectors.len(), 1);
            assert_eq!(connectors[0].position, at);
            assert_eq!(connectors[0].orientation, Orientation::parse("down_south")?);
            assert_eq!(connectors[0].joint, Joint::Rollable);
            assert!(connectors[0].name.is_none());
            assert_eq!(
                context.bounds(&element, at, Rotation::Clockwise90)?,
                BoundingBox::new(at, at).unwrap()
            );
            assert_eq!(context.surface_height(at)?, 64);
        }
        assert_eq!(provider.reads, 1);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn trial_aliases_resolve_against_active_template_pools() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let bytes = resources
            .read_data("data/minecraft/worldgen/structure/trial_chambers.json")?
            .context("trial chambers definition missing")?;
        let definition: serde_json::Value = serde_json::from_slice(&bytes)?;
        let aliases = PoolAliases::decode(&definition["pool_aliases"])?;
        let position = [-48, -27, 32];
        let expected = aliases.resolve(42, position)?;
        assert_eq!(expected.len(), 4);
        let mut context = TemplateAssemblyContext::for_structure_start(
            &mut resources,
            |_| Ok(64),
            &aliases,
            42,
            position,
        )?;
        for (alias, target) in expected {
            assert_eq!(context.resolve_alias(&alias)?, target);
            assert!(
                context.pool(&target)?.is_some(),
                "resolved trial pool is missing"
            );
        }
        Ok(())
    }

    fn vanilla_graph_lines(
        resources: &mut native_resources::packs::ResourceStack,
        seed: i64,
    ) -> Result<VecDeque<String>> {
        use native_resources::structure_pool::Projection;
        let mut context = TemplateAssemblyContext::new(resources, |_| Ok(-27), BTreeMap::new());
        let pool = context
            .pool(&ResourceId::parse("minecraft:ancient_city/city_center")?)?
            .context("city center pool missing")?;
        let element = pool.entries()[0].0.clone();
        let origin = [0, -27, 0];
        let root = Piece {
            bounds: context.bounds(&element, origin, Rotation::None)?,
            element,
            origin,
            rotation: Rotation::None,
            ground_level_delta: 1,
            junctions: vec![],
        };
        let mut random = native_core::random::LegacyRandom::new(seed);
        let pieces = assemble(
            &mut context,
            root,
            BoundingBox::new([-128, -64, -128], [128, 100, 128]).unwrap(),
            2,
            false,
            &mut |bound| random.next_int_bound(bound),
        )?;
        let mut result = VecDeque::new();
        result.push_back(format!(
            "C\t{seed}\t{}\t{}",
            pieces.len(),
            random.next_long()
        ));
        for piece in pieces {
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
                let projection = match j.destination_projection {
                    Projection::Rigid => "rigid",
                    Projection::TerrainMatching => "terrain_matching",
                };
                result.push_back(format!(
                    "J\t{}\t{}\t{}\t{}\t{projection}",
                    j.source_x, j.source_ground_y, j.source_z, j.delta_y
                ));
            }
        }
        Ok(result)
    }

    fn configured_city_lines(
        resources: &mut native_resources::packs::ResourceStack,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<VecDeque<String>> {
        use native_resources::{
            height_provider::HeightContext,
            jigsaw_placement::{StartEnvironment, StartHeightmap},
            jigsaw_structure::JigsawStructure,
            structure_pool::Projection,
        };
        struct Terrain;
        impl StartEnvironment for Terrain {
            fn valid_column(&mut self, _: i32, _: i32, _: i32, _: i32) -> Result<bool> {
                anyhow::bail!("city unexpectedly projected to terrain")
            }
            fn first_free_height(&mut self, _: i32, _: i32, _: StartHeightmap) -> Result<i32> {
                anyhow::bail!("city unexpectedly sampled start terrain")
            }
        }
        let bytes = resources
            .read_data("data/minecraft/worldgen/structure/ancient_city.json")?
            .context("missing city definition")?;
        let mut definition: serde_json::Value = serde_json::from_slice(&bytes)?;
        definition["size"] = serde_json::json!(2);
        let settings = JigsawStructure::decode(&definition)?;
        let mut seeded = settings.begin(
            seed,
            chunk,
            HeightContext {
                min_y: 0,
                height: 0,
                sea_level: 0,
            },
        )?;
        let mut context =
            TemplateAssemblyContext::new(resources, |_| Ok(-27), seeded.aliases.clone());
        let prepared = seeded
            .prepare(&settings, &mut context, &mut Terrain, -64, 319)?
            .context("missing city start")?;
        let point = prepared.position;
        let pieces = prepared.assemble(&mut context, &mut |bound| {
            seeded.random.next_int_bound(bound)
        })?;
        let mut result = VecDeque::new();
        result.push_back(format!(
            "C\t{seed}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            chunk[0],
            chunk[1],
            point[0],
            point[1],
            point[2],
            pieces.len(),
            seeded.random.next_long()
        ));
        for piece in pieces {
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
                let projection = match j.destination_projection {
                    Projection::Rigid => "rigid",
                    Projection::TerrainMatching => "terrain_matching",
                };
                result.push_back(format!(
                    "J\t{}\t{}\t{}\t{}\t{projection}",
                    j.source_x, j.source_ground_y, j.source_z, j.delta_y
                ));
            }
        }
        Ok(result)
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn configured_city_start_and_graph_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
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
            .context("Java 25 home missing")?
            .join("bin");
        let shared = temp.path().join("JigsawPlacementOracle.java");
        let source = temp.path().join("JigsawConfiguredCityOracle.java");
        std::fs::write(
            &shared,
            include_str!("../../resources/tests/java/JigsawPlacementOracle.java"),
        )?;
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawConfiguredCityOracle.java"),
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
                ensure!(
                    expected.is_empty(),
                    "previous configured graph was incomplete"
                );
                let fields: Vec<_> = rest.split('\t').collect();
                ensure!(fields.len() == 8, "invalid configured graph header");
                expected = configured_city_lines(
                    &mut resources,
                    fields[0].parse()?,
                    [fields[1].parse()?, fields[2].parse()?],
                )?;
                cases += 1;
            }
            if line.starts_with("C\t") || line.starts_with("P\t") || line.starts_with("J\t") {
                assert_eq!(
                    expected.pop_front().context("extra Java graph output")?,
                    line,
                    "configured graph case {cases}"
                );
            }
        }
        assert_eq!(cases, 6);
        assert!(expected.is_empty());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn active_ancient_city_templates_form_a_piece_graph() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let mut context =
            TemplateAssemblyContext::new(&mut resources, |_| Ok(-27), BTreeMap::new());
        let pool_id = ResourceId::parse("minecraft:ancient_city/city_center")?;
        let pool = context
            .pool(&pool_id)?
            .context("city center pool missing")?;
        let cached = context.pool(&pool_id)?.unwrap();
        assert!(Arc::ptr_eq(&pool, &cached));
        let element = pool.entries()[0].0.clone();
        let origin = [0, -27, 0];
        let bounds = context.bounds(&element, origin, Rotation::None)?;
        let root = Piece {
            element,
            origin,
            rotation: Rotation::None,
            bounds,
            ground_level_delta: 1,
            junctions: vec![],
        };
        let mut random = native_core::random::LegacyRandom::new(42);
        let pieces = assemble(
            &mut context,
            root,
            BoundingBox::new([-128, -64, -128], [128, 100, 128]).unwrap(),
            2,
            false,
            &mut |bound| random.next_int_bound(bound),
        )?;
        assert!(pieces.len() > 1, "no city pieces were connected");
        assert!(!pieces[0].junctions.is_empty());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn vanilla_ancient_city_piece_graph_matches_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
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
            .context("Java 25 home missing")?
            .join("bin");
        let shared = temp.path().join("JigsawPlacementOracle.java");
        let source = temp.path().join("JigsawVanillaGraphOracle.java");
        std::fs::write(
            &shared,
            include_str!("../../resources/tests/java/JigsawPlacementOracle.java"),
        )?;
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawVanillaGraphOracle.java"),
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
            .arg("JigsawVanillaGraphOracle")
            .arg(version.join("26.3.jar"))
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut expected = VecDeque::new();
        let mut cases = 0;
        for line in String::from_utf8(output.stdout)?.lines() {
            if let Some(rest) = line.strip_prefix("C\t") {
                ensure!(expected.is_empty(), "previous Java graph was incomplete");
                let fields: Vec<_> = rest.split('\t').collect();
                ensure!(fields.len() == 3, "invalid Java graph header");
                expected = vanilla_graph_lines(&mut resources, fields[0].parse()?)?;
                cases += 1;
            }
            if line.starts_with("C\t") || line.starts_with("P\t") || line.starts_with("J\t") {
                let rust = expected.pop_front().context("extra Java graph output")?;
                assert_eq!(rust, line, "vanilla graph mismatch in case {cases}");
            }
        }
        assert_eq!(cases, 3);
        assert!(expected.is_empty());
        Ok(())
    }
}
