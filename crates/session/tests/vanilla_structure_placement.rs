use anyhow::{Context, Result, ensure};

use native_resources::packs::ResourceStack;
use native_world::loaded::BlockState;
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn structure_placement_matches_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
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
    let source = temp.path().join("StructurePlacementOracle.java");
    std::fs::write(&source, include_str!("java/StructurePlacementOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("StructurePlacementOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    use native_core::{
        beardifier::{BoundingBox, TerrainAdjustment},
        random::DecorationRandom,
    };
    use native_persistence::{
        nbt::{NamedTag, Tag},
        structure_processors::PreparedProcessors,
        structure_template::{StructureTemplate, TemplateBlock},
    };
    use native_resources::{
        packs::ResourceId,
        ruined_portal::{GenerationPoint, InitialSelection, Setup, VerticalPlacement},
        structure_pool::HolderValue,
        structure_transform::{Mirror, Rotation, bounding_box},
        tree_leaf_update::ShapeEdgeWorld,
    };
    use native_session::worldgen::jigsaw::{JigsawPlacementWorld, StructurePlacementWorld};
    use native_session::worldgen::ruined_portal::{PreparedStart, SavedRuinedPortalPiece};
    use native_world::fluid::FluidState;
    fn key(s: &BlockState) -> String {
        let mut out = format!("{}|", s.name);
        for (k, v) in &s.properties {
            out.push_str(&format!("{k}={v};"));
        }
        out
    }
    fn air() -> BlockState {
        BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        }
    }
    #[derive(Default)]
    struct World {
        states: BTreeMap<[i32; 3], BlockState>,
        events: Vec<String>,
    }
    impl JigsawPlacementWorld for World {
        fn block(&mut self, p: [i32; 3]) -> Result<BlockState> {
            Ok(self.states.get(&p).cloned().unwrap_or_else(air))
        }
        fn fluid_at(&mut self, _: [i32; 3]) -> Result<Option<FluidState>> {
            Ok(None)
        }
        fn fluid_of_state(&mut self, _: &BlockState) -> Result<Option<FluidState>> {
            Ok(None)
        }
        fn place_template_block(
            &mut self,
            _: [i32; 3],
            _: &BlockState,
            _: Option<&Tag>,
            _: &mut DecorationRandom,
        ) -> Result<bool> {
            anyhow::bail!("unexpected jigsaw write")
        }
        fn is_liquid_container(&mut self, s: &BlockState) -> Result<bool> {
            Ok(s.name == "minecraft:iron_bars")
        }
        fn place_liquid(
            &mut self,
            _: [i32; 3],
            _: &BlockState,
            _: Option<FluidState>,
        ) -> Result<()> {
            Ok(())
        }
        fn mark_block_entity_changed(&mut self, _: [i32; 3]) -> Result<()> {
            anyhow::bail!("unexpected block entity")
        }
        fn place_template_entities(
            &mut self,
            _: &[Tag],
            _: [i32; 3],
            _: Rotation,
            _: BoundingBox,
        ) -> Result<()> {
            anyhow::bail!("unexpected entity")
        }
    }
    impl ShapeEdgeWorld for World {
        type State = BlockState;
        fn block(&mut self, p: [i32; 3]) -> Result<BlockState> {
            JigsawPlacementWorld::block(self, p)
        }
        fn update_shape(
            &mut self,
            _: [i32; 3],
            state: &BlockState,
            d: [i32; 3],
            _: [i32; 3],
            neighbor: &BlockState,
        ) -> Result<BlockState> {
            let mut result = state.clone();
            if state.name == "minecraft:iron_bars" {
                let face = match d {
                    [0, 0, -1] => Some("north"),
                    [1, 0, 0] => Some("east"),
                    [0, 0, 1] => Some("south"),
                    [-1, 0, 0] => Some("west"),
                    _ => None,
                };
                if let Some(face) = face {
                    result.properties.insert(
                        face.into(),
                        matches!(
                            neighbor.name.as_str(),
                            "minecraft:stone" | "minecraft:iron_bars"
                        )
                        .to_string(),
                    );
                }
            }
            Ok(result)
        }
        fn set_block(&mut self, p: [i32; 3], s: &BlockState, flags: i32) -> Result<()> {
            self.events.push(format!(
                "W\t{}\t{}\t{}\t{}\t{}",
                p[0],
                p[1],
                p[2],
                key(s),
                flags
            ));
            self.states.insert(p, s.clone());
            Ok(())
        }
    }
    impl StructurePlacementWorld for World {
        fn place_structure_block(
            &mut self,
            p: [i32; 3],
            s: &BlockState,
            _: Option<&Tag>,
            _: &mut DecorationRandom,
        ) -> Result<bool> {
            ShapeEdgeWorld::set_block(self, p, s, 2)?;
            Ok(true)
        }
        fn update_from_neighbor_shapes(
            &mut self,
            p: [i32; 3],
            s: &BlockState,
        ) -> Result<BlockState> {
            let mut result = s.clone();
            for d in [
                [0, -1, 0],
                [0, 1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [-1, 0, 0],
                [1, 0, 0],
            ] {
                let n = std::array::from_fn(|i| p[i] + d[i]);
                let other = ShapeEdgeWorld::block(self, n)?;
                result = ShapeEdgeWorld::update_shape(self, p, &result, d, n, &other)?;
            }
            Ok(result)
        }
        fn update_neighbors_at(&mut self, p: [i32; 3], _: &BlockState) -> Result<()> {
            self.events.push(format!("N\t{}\t{}\t{}", p[0], p[1], p[2]));
            Ok(())
        }
    }
    let mut data = ResourceStack::open(&game)?;
    let registry = data.vanilla_generated_block_registry()?;
    let blocks = data.vanilla_block_ids()?;
    let processors = PreparedProcessors::load(
        &HolderValue::Inline(serde_json::json!({"processors":[]})),
        &mut data,
        &blocks,
        &registry,
    )?;
    let positions = [
        ([0, 0, 0], 0),
        ([2, 0, 0], 0),
        ([1, 0, 0], 1),
        ([1, 0, 1], 1),
        ([3, 1, 1], 1),
    ];
    let mirrors = [Mirror::None, Mirror::LeftRight, Mirror::FrontBack];
    let rotations = [
        Rotation::None,
        Rotation::Clockwise90,
        Rotation::Clockwise180,
        Rotation::Counterclockwise90,
    ];
    let mut events: Vec<String> = Vec::new();
    let mut index = 0;
    let mut next = 0;
    let mut count = 0;
    for line in String::from_utf8(output.stdout)?.lines() {
        let f: Vec<_> = line.split('\t').collect();
        match f.as_slice() {
            ["CASE", m, r, n] => {
                ensure!(index == events.len(), "missing expected events");
                let mirror = mirrors[m.parse::<usize>()?];
                let rotation = rotations[r.parse::<usize>()?];
                let origin = [4, 12, -6];
                let pivot = [2, 0, 2];
                let size = [4, 2, 4];
                let bounds = bounding_box(size, origin, mirror, rotation, pivot)?;
                let point = GenerationPoint {
                    position: origin,
                    pivot,
                    bounds,
                };
                let selection = InitialSelection {
                    setup_index: 0,
                    air_pocket: false,
                    template: ResourceId::parse("minecraft:ruined_portal/portal_1")?,
                    mirror,
                    rotation,
                };
                let setup = Setup {
                    placement: VerticalPlacement::OnLandSurface,
                    air_pocket_probability: 0.0,
                    mossiness: 0.0,
                    overgrown: false,
                    vines: false,
                    can_be_cold: false,
                    replace_with_blackstone: false,
                    weight: 1.0,
                };
                let piece =
                    SavedRuinedPortalPiece::capture(&selection, &setup, &point, size, false)?;
                let template = StructureTemplate {
                    document: NamedTag {
                        name: "".into(),
                        value: Tag::Compound(vec![]),
                    },
                    data_version: Some(5023),
                    size,
                    palettes: vec![vec![
                        Tag::Compound(vec![("id".into(), Tag::String("minecraft:stone".into()))]),
                        Tag::Compound(vec![(
                            "id".into(),
                            Tag::String("minecraft:iron_bars".into()),
                        )]),
                    ]],
                    blocks: positions
                        .iter()
                        .map(|(position, state)| TemplateBlock {
                            position: *position,
                            state: *state,
                            nbt: None,
                        })
                        .collect(),
                    entities: vec![],
                };
                let start = PreparedStart {
                    chunk: [0, 0],
                    point,
                    terrain_adjustment: TerrainAdjustment::None,
                    template: std::sync::Arc::new(template),
                    piece,
                };
                let mut world = World::default();
                let mut random = DecorationRandom::new(42);
                assert!(start.place_template(
                    &mut world,
                    &registry,
                    &processors,
                    BoundingBox {
                        min: [-20, -20, -20],
                        max: [20, 30, 20],
                    },
                    |state| Ok(state.name == "minecraft:stone"),
                    &mut random,
                )?);
                assert_eq!(world.events.len(), n.parse::<usize>()?, "{line}");
                events = world.events;
                index = 0;
                next = random.next_long();
                count += 1;
            }
            ["W", ..] | ["N", ..] => {
                assert_eq!(
                    events.get(index).context("too many Java events")?,
                    line,
                    "event {index}"
                );
                index += 1;
            }
            ["END", v] => {
                assert_eq!(index, events.len());
                assert_eq!(next, v.parse::<i64>()?);
            }
            _ => {}
        }
    }
    assert_eq!(count, 12);
    Ok(())
}
