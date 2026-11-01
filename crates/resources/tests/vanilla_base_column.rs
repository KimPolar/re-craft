//! Base-noise columns and structure heightmaps against the installed Java generator.
use native_resources::{
    density::DensityEvaluator,
    noise_stage::{NoiseMaterial, NoiseStageGenerator},
    packs::ResourceStack,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java base-column oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn id(v: &Value) -> &str {
    v.as_str()
        .or_else(|| v.get("id").and_then(Value::as_str))
        .unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn base_columns_and_heightmaps_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("BaseColumnOracle.java");
    std::fs::write(&source, include_str!("java/BaseColumnOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BaseColumnOracle"));
    let mut resources = ResourceStack::open(&game).unwrap();
    let mut current = None;
    let mut evaluator = DensityEvaluator::default();
    let mut count = 0;
    for line in output.lines().filter(|line| line.starts_with("P ")) {
        let words: Vec<_> = line.split_whitespace().collect();
        let dim: usize = words[1].parse().unwrap();
        let seed: i64 = words[2].parse().unwrap();
        if current
            .as_ref()
            .is_none_or(|(d, s, _)| (*d, *s) != (dim, seed))
        {
            let name = ["minecraft:overworld", "minecraft:nether", "minecraft:end"][dim];
            current = Some((
                dim,
                seed,
                NoiseStageGenerator::load_without_terrain_adjustments(&mut resources, name, seed)
                    .unwrap(),
            ));
        }
        let generator = &current.as_ref().unwrap().2;
        let x: i32 = words[3].parse().unwrap();
        let z: i32 = words[4].parse().unwrap();
        let min: i32 = words[5].parse().unwrap();
        let height: usize = words[6].parse().unwrap();
        let column = generator.base_column(&mut evaluator, x, z).unwrap();
        assert_eq!(column.min_y(), min, "{line}");
        assert_eq!(column.materials().len(), height, "{line}");
        let code = |state| match state {
            NoiseMaterial::Air => 'A',
            NoiseMaterial::Lava => 'L',
            NoiseMaterial::DefaultBlock => match id(generator.default_block()) {
                "minecraft:stone" => 'S',
                "minecraft:netherrack" => 'N',
                "minecraft:end_stone" => 'E',
                other => panic!("{other}"),
            },
            NoiseMaterial::DefaultFluid => match id(generator.default_fluid()) {
                "minecraft:air" => 'A',
                "minecraft:water" => 'W',
                "minecraft:lava" => 'L',
                other => panic!("{other}"),
            },
        };
        let actual: String = column.materials().iter().copied().map(code).collect();
        assert_eq!(actual, words[9], "dimension={dim} seed={seed} x={x} z={z}");
        let dim_min = if dim == 0 { -64 } else { 0 };
        let surface = column
            .first_occupied_height(dim_min, |v| Ok(code(v) != 'A'))
            .unwrap();
        let floor = column
            .first_occupied_height(dim_min, |v| Ok(matches!(code(v), 'S' | 'N' | 'E')))
            .unwrap();
        assert_eq!(surface, words[7].parse::<i32>().unwrap(), "{line}");
        assert_eq!(floor, words[8].parse::<i32>().unwrap(), "{line}");
        count += 1;
    }
    assert_eq!(count, 48);
}
