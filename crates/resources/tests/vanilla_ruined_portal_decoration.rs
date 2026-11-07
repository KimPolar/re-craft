use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ruined_portal_decoration_matches_java() -> anyhow::Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("RuinedPortalDecorationOracle.java");
    std::fs::write(
        &source,
        include_str!("java/RuinedPortalDecorationOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    anyhow::ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("RuinedPortalDecorationOracle")
        .arg(version.join("26.3.jar"))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    use anyhow::{Context, Result};
    use native_core::{beardifier::BoundingBox, random::DecorationRandom};
    use native_resources::{
        ruined_portal::VerticalPlacement,
        ruined_portal_decoration::{PortalBlock, PortalDecoration, PortalDecorationWorld},
        structure_start::Heightmap,
    };
    #[derive(Default)]
    struct World {
        changes: BTreeMap<[i32; 3], String>,
        events: Vec<String>,
    }
    fn surface(x: i32, z: i32, map: Heightmap) -> i32 {
        63 + (x * 3 + z * 5).rem_euclid(7) - if map == Heightmap::OceanFloor { 2 } else { 0 }
    }
    fn initial([x, y, z]: [i32; 3]) -> String {
        let name = if y > surface(x, z, Heightmap::WorldSurface) {
            "air"
        } else {
            match (x * 31 + y * 13 + z * 17).rem_euclid(19) {
                0 => "lava",
                1 => "obsidian",
                2 => "bedrock",
                3 => "cave_air",
                4 => "netherrack",
                _ => "stone",
            }
        };
        match name {
            "lava" => "minecraft:lava|level=0;".into(),
            _ => format!("minecraft:{name}|"),
        }
    }
    impl PortalDecorationWorld for World {
        type State = String;
        fn block(&mut self, p: [i32; 3]) -> Result<String> {
            Ok(self.changes.get(&p).cloned().unwrap_or_else(|| initial(p)))
        }
        fn id<'a>(&self, s: &'a String) -> &'a str {
            s.split_once('|').unwrap().0
        }
        fn is_air(&self, s: &String) -> bool {
            matches!(
                self.id(s),
                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            )
        }
        fn is_protected(&self, s: &String) -> Result<bool> {
            Ok(self.id(s) == "minecraft:bedrock")
        }
        fn height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32> {
            Ok(surface(x, z, map) + 1)
        }
        fn collision_face_full(&mut self, p: [i32; 3], _: [i32; 3]) -> Result<bool> {
            let s = self.block(p)?;
            Ok(!self.is_air(&s) && !matches!(self.id(&s), "minecraft:lava" | "minecraft:vine"))
        }
        fn set_block(&mut self, p: [i32; 3], b: PortalBlock) -> Result<()> {
            let key = match b {
                PortalBlock::Netherrack => "minecraft:netherrack|".into(),
                PortalBlock::Magma => "minecraft:magma_block|".into(),
                PortalBlock::PersistentJungleLeaves => {
                    "minecraft:jungle_leaves|distance=7;persistent=true;waterlogged=false;".into()
                }
                PortalBlock::Vine(d) => {
                    let face = match d {
                        [0, 0, -1] => "north",
                        [1, 0, 0] => "east",
                        [0, 0, 1] => "south",
                        [-1, 0, 0] => "west",
                        _ => anyhow::bail!("invalid vine direction"),
                    };
                    format!(
                        "minecraft:vine|east={};north={};south={};up=false;west={};",
                        face == "east",
                        face == "north",
                        face == "south",
                        face == "west"
                    )
                }
            };
            self.events
                .push(format!("B\t{}\t{}\t{}\t{}", p[0], p[1], p[2], key));
            self.changes.insert(p, key);
            Ok(())
        }
    }
    let placements = [
        VerticalPlacement::OnLandSurface,
        VerticalPlacement::PartlyBuried,
        VerticalPlacement::OnOceanFloor,
        VerticalPlacement::InMountain,
        VerticalPlacement::Underground,
        VerticalPlacement::InNether,
    ];
    let mut events: Vec<String> = Vec::new();
    let mut index = 0;
    let mut next = 0;
    let mut cases = 0;
    for line in String::from_utf8(output.stdout)?.lines() {
        let f: Vec<_> = line.split('\t').collect();
        match f.as_slice() {
            ["CASE", placement, flags, seed, count] => {
                anyhow::ensure!(index == events.len(), "missing event");
                let flags: i32 = flags.parse()?;
                let mut world = World::default();
                let mut random = DecorationRandom::new(seed.parse()?);
                PortalDecoration {
                    bounds: BoundingBox {
                        min: [-3, 64, -2],
                        max: [3, 67, 2],
                    },
                    placement: placements[placement.parse::<usize>()?],
                    cold: flags & 1 != 0,
                    overgrown: flags & 2 != 0,
                    vines: flags & 4 != 0,
                }
                .apply(&mut world, &mut random)?;
                assert_eq!(world.events.len(), count.parse::<usize>()?, "{line}");
                events = world.events;
                index = 0;
                next = random.next_long();
                cases += 1;
            }
            ["B", ..] => {
                assert_eq!(
                    events.get(index).context("extra Java event")?,
                    line,
                    "event {index}"
                );
                index += 1;
            }
            ["END", value] => {
                assert_eq!(index, events.len());
                assert_eq!(next, value.parse::<i64>()?, "random state");
            }
            _ => {}
        }
    }
    assert_eq!(cases, 6 * 8 * 3);
    Ok(())
}
