use super::*;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn double_plant_above_leaves_matches_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut resources = native_resources::packs::ResourceStack::open(&game)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let names = resources.vanilla_block_ids()?;
    let motion = HeightmapTags::load(&mut resources, &names)?;
    let required = BTreeSet::from([ResourceId::parse("minecraft:supports_vegetation")?]);
    let tags = OreBlockTags::load(&mut super::tests::Data, &names, &required)?;
    let feature = SimpleBlockFeature::decode(
        &json!({"type":"minecraft:simple_block", "to_place":{"id":"minecraft:tall_grass"}}),
        &mut |value| {
            let (name, properties) = registry.resolve(value)?;
            Ok(BlockState { name, properties })
        },
    )?;
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
    let cp = std::env::join_paths(paths)?;
    let source = temp.path().join("DoublePlantLeafOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/DoublePlantLeafOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("DoublePlantLeafOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut count = 0;
    for line in String::from_utf8(output.stdout)?.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let ["L", name, distance, persistent, waterlogged, expected, next] = fields.as_slice()
        else {
            continue;
        };
        let leaf = BlockState {
            name: (*name).into(),
            properties: BTreeMap::from([
                ("distance".into(), (*distance).into()),
                ("persistent".into(), (*persistent).into()),
                ("waterlogged".into(), (*waterlogged).into()),
            ]),
        };
        let (mut terrain, _, _, _) = super::tests::fixture();
        let chunks = terrain.snapshot().chunks.keys().copied().collect();
        let mut setup = terrain.decoration_region(&tags, &motion, chunks)?;
        setup.set_proto_block([8, 9, 8], &leaf)?;
        setup.finish()?;
        let mut region =
            terrain.decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))?;
        let mut random = DecorationRandom::new(42);
        assert_eq!(
            feature.place(&mut region, &mut random, [8, 8, 8])?,
            expected.parse::<bool>()?,
            "{line}"
        );
        assert_eq!(random.next_long(), next.parse::<i64>()?, "{line}");
        region.finish()?;
        let world = terrain.snapshot();
        assert_eq!(world.block(8, 9, 8), Some(&leaf), "{line}");
        assert_eq!(
            world.block(8, 8, 8).map(|state| state.name.as_str()),
            Some("minecraft:air"),
            "{line}"
        );
        count += 1;
    }
    assert_eq!(count, 5 * 7 * 2 * 2);
    Ok(())
}
