use super::*;
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn kelp_column_matches_java_feature_and_random_state() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")
        })
        .join("bin");
    let mut resources = native_resources::packs::ResourceStack::open(&game)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let names = resources.vanilla_block_ids()?;
    let motion = HeightmapTags::load(&mut resources, &names)?;
    let value: Value = serde_json::from_slice(
        &resources
            .read_data("data/minecraft/worldgen/feature/kelp.json")?
            .context("missing kelp feature")?,
    )?;
    let feature = BlockColumnFeature::decode_with_resources(&mut resources, &value, &mut |v| {
        let (name, properties) = registry.resolve(v)?;
        Ok(BlockState { name, properties })
    })?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong Minecraft version");
    let temp = tempfile::tempdir()?;
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_owned()];
    for library in manifest["libraries"]
        .as_array()
        .context("missing libraries")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("KelpColumnOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/KelpColumnOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg("KelpColumnOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut count = 0;
    for line in String::from_utf8(output.stdout)?.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let ["K", depth, level, restricted, seed, expected, next, changes] = fields.as_slice()
        else {
            continue;
        };
        let depth: i32 = depth.parse()?;
        let restricted: bool = restricted.parse()?;
        let (mut terrain, tags, _, _) = super::tests::fixture();
        let all_chunks = terrain.snapshot().chunks.keys().copied().collect();
        let mut setup = terrain.decoration_region(&tags, &motion, all_chunks)?;
        for y in 2..16 {
            let block = if y < 2 + depth {
                BlockState {
                    name: "minecraft:water".into(),
                    properties: BTreeMap::from([("level".into(), (*level).into())]),
                }
            } else {
                BlockState {
                    name: "minecraft:air".into(),
                    properties: BTreeMap::new(),
                }
            };
            setup.set_proto_block([8, y, 8], &block)?;
        }
        setup.finish()?;
        let writable = if restricted {
            BTreeSet::new()
        } else {
            BTreeSet::from([ChunkPos { x: 0, z: 0 }])
        };
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(seed.parse()?);
        assert_eq!(
            feature.place(&mut region, &mut random, [8, 2, 8])?,
            expected.parse::<bool>()?,
            "{line}"
        );
        assert_eq!(random.next_long(), next.parse::<i64>()?, "{line}");
        region.finish()?;
        let world = terrain.snapshot();
        let mut actual = Vec::new();
        for y in 2..16 {
            let block = world.block(8, y, 8).context("missing kelp position")?;
            if matches!(
                block.name.as_str(),
                "minecraft:kelp" | "minecraft:kelp_plant"
            ) {
                actual.push(format!(
                    "{y}:{}:{}",
                    block.name,
                    block
                        .properties
                        .get("age")
                        .map(String::as_str)
                        .unwrap_or("-")
                ));
            }
        }
        assert_eq!(
            if actual.is_empty() {
                "-".into()
            } else {
                actual.join(",")
            },
            *changes,
            "{line}"
        );
        count += 1;
    }
    assert_eq!(count, 12 * 2 * 2 * 5);
    Ok(())
}
