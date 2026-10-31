//! Differential checks against the locally installed 26.3 density sampler.
use native_resources::{density::DensityCompiler, packs::ResourceProvider};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

struct Empty;
impl ResourceProvider for Empty {
    fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).unwrap()
}

fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "Java density oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn pure_functions_match_java_sampler() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("DensityOracle.java");
    std::fs::write(&source, include_str!("java/DensityOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let gradient = json!({"type":"minecraft:gradient","axis":"y","from_coordinate":-64,"to_coordinate":320,"from_value":1.5,"to_value":-1.5});
    let reverse = json!({"type":"minecraft:gradient","axis":"x","from_coordinate":40,"to_coordinate":-20,"from_value":-0.8,"to_value":0.7});
    let mut expressions = vec![json!(0.25), gradient.clone(), reverse.clone()];
    for kind in ["add", "sub", "mul"] {
        expressions
            .push(json!({"type":format!("minecraft:{kind}"), "left":gradient, "right":reverse}));
        expressions
            .push(json!({"type":format!("minecraft:{kind}"), "left":-0.25, "right":gradient}));
    }
    for kind in [
        "abs",
        "square",
        "cube",
        "half_negative",
        "quarter_negative",
        "negate",
        "squeeze",
    ] {
        expressions.push(json!({"type":format!("minecraft:{kind}"), "input":gradient}));
    }
    expressions.push(json!({"type":"minecraft:range_choice", "input":gradient, "min_inclusive":-0.5, "max_exclusive":0.5, "when_in_range":reverse, "when_out_of_range":0.125}));
    let zero = json!({"type":"minecraft:gradient", "axis":"y", "from_coordinate":0, "to_coordinate":1, "from_value":0.0, "to_value":0.0});
    let negative = json!({"type":"minecraft:negate", "input":1.0});
    expressions.push(json!({"type":"minecraft:mul", "left":zero, "right":negative}));
    expressions.push(json!({"type":"minecraft:mul", "left":zero, "right":-1.0}));
    expressions.push(json!({"type":"minecraft:sub", "left":zero, "right":0.0}));
    for alpha in [json!(0.0), json!(1.0), json!(0.375), gradient.clone()] {
        expressions.push(
            json!({"type":"minecraft:lerp", "alpha":alpha, "first":gradient, "second":reverse}),
        );
        expressions
            .push(json!({"type":"minecraft:lerp", "alpha":alpha, "first":-0.75, "second":reverse}));
        expressions
            .push(json!({"type":"minecraft:lerp", "alpha":alpha, "first":gradient, "second":0.25}));
    }
    let nonlinear = json!({"type":"minecraft:mul", "left":{"type":"minecraft:square", "input":gradient}, "right":reverse});
    for (cell_xz, cell_y) in [(4, 8), (3, 5), (1, 1), (9, 7)] {
        expressions.push(json!({"type":"minecraft:interpolated", "cell_size_xz":cell_xz, "cell_size_y":cell_y, "input":nonlinear}));
    }
    expressions.push(json!({"type":"minecraft:interpolated", "cell_size_xz":4, "cell_size_y":8, "input":{"type":"minecraft:mul", "left":zero, "right":negative}}));
    expressions.push(json!({"type":"minecraft:interpolated", "cell_size_xz":4, "cell_size_y":8, "input":{"type":"minecraft:lerp", "alpha":gradient, "first":nonlinear, "second":reverse}}));
    let input = temp.path().join("expressions.json");
    std::fs::write(&input, serde_json::to_vec(&expressions).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("DensityOracle")
        .arg(input));
    let mut provider = Empty;
    let mut compiler = DensityCompiler::new(&mut provider);
    let compiled: Vec<_> = expressions
        .iter()
        .map(|v| compiler.compile(v).unwrap())
        .collect();
    let mut count = 0;
    for line in output.lines() {
        let parts: Vec<i32> = line
            .split_whitespace()
            .map(|p| p.parse().unwrap())
            .collect();
        assert_eq!(parts.len(), 5, "unexpected Java output: {line}");
        let [index, x, y, z, bits]: [i32; 5] = parts.try_into().unwrap();
        assert_eq!(
            compiled[index as usize].sample(x, y, z).to_bits(),
            bits as u32,
            "expression {index} at {x} {y} {z}: {}",
            expressions[index as usize]
        );
        count += 1;
    }
    assert_eq!(count, expressions.len() * 52);
}
