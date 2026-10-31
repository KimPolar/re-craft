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
    for (left, right) in [
        (gradient.clone(), reverse.clone()),
        (json!(1.0), gradient.clone()),
        (gradient.clone(), json!(3.0)),
        (json!(-2.0), json!(0.0)),
        (zero.clone(), reverse.clone()),
        (gradient.clone(), json!(-0.0)),
    ] {
        expressions.push(json!({"type":"div", "left":left, "right":right}));
        expressions.push(json!({"type":"interpolated", "cell_size_xz":4, "cell_size_y":8, "input":{"type":"div", "left":left, "right":right}}));
    }
    for thresholds in [vec![0.0], vec![-0.5, 0.0, 0.0, 0.5]] {
        let functions = (0..=thresholds.len())
            .map(|i| json!(i as f32 * 0.5 - 1.0))
            .collect::<Vec<_>>();
        let select = json!({"type":"interval_select", "input":gradient, "thresholds":thresholds, "functions":functions});
        expressions.push(select.clone());
        expressions.push(
            json!({"type":"interpolated", "cell_size_xz":4, "cell_size_y":8, "input":select}),
        );
    }
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
    let infinity = json!({"type":"square", "input":{"type":"square", "input":{"type":"square", "input":1000000.0}}});
    let negative_infinity = json!({"type":"negate", "input":infinity});
    let z_gradient = json!({"type":"gradient","axis":"z","from_coordinate":-20,"to_coordinate":40,"from_value":-1,"to_value":1});
    for varying in [
        json!(0),
        gradient.clone(),
        reverse.clone(),
        z_gradient.clone(),
        json!({"type":"add","left":gradient,"right":reverse}),
        json!({"type":"add","left":reverse,"right":z_gradient}),
        json!({"type":"add","left":gradient,"right":z_gradient}),
    ] {
        let interpolated = json!({"type":"interpolated", "cell_size_xz":4,"cell_size_y":8,"input":{"type":"add","left":infinity,"right":varying}});
        expressions.push(interpolated.clone());
        expressions.push(json!({"type":"add", "left":interpolated, "right":z_gradient}));
    }
    expressions.push(infinity.clone());
    for alpha in [json!(-0.5), json!(0.5), json!(1.5)] {
        expressions.push(
            json!({"type":"lerp", "alpha":alpha, "first":infinity, "second":negative_infinity}),
        );
    }
    for kind in ["min", "max"] {
        for (left, right) in [
            (gradient.clone(), reverse.clone()),
            (json!(0.25), gradient.clone()),
            (gradient.clone(), json!(0.25)),
            (json!(-3.0), gradient.clone()),
            (gradient.clone(), json!(3.0)),
            (zero.clone(), negative.clone()),
            (json!(-0.0), json!(0.0)),
            (nonlinear.clone(), gradient.clone()),
            (infinity.clone(), negative_infinity.clone()),
            (infinity.clone(), zero.clone()),
            (zero.clone(), json!({"type":"negate", "input":0.0})),
            (json!({"type":"negate", "input":0.0}), zero.clone()),
        ] {
            expressions
                .push(json!({"type":format!("minecraft:{kind}"), "left":left, "right":right}));
        }
        expressions.push(json!({"type":"minecraft:interpolated", "cell_size_xz":4, "cell_size_y":8, "input":{"type":format!("minecraft:{kind}"), "left":gradient, "right":reverse}}));
    }
    for (cell_xz, cell_y) in [(4, 8), (3, 5), (1, 1), (9, 7)] {
        expressions.push(json!({"type":"minecraft:interpolated", "cell_size_xz":cell_xz, "cell_size_y":cell_y, "input":nonlinear}));
    }
    expressions.push(json!({"type":"minecraft:interpolated", "cell_size_xz":4, "cell_size_y":8, "input":{"type":"minecraft:mul", "left":zero, "right":negative}}));
    expressions.push(json!({"type":"minecraft:interpolated", "cell_size_xz":4, "cell_size_y":8, "input":{"type":"minecraft:lerp", "alpha":gradient, "first":nonlinear, "second":reverse}}));
    for (min, max) in [(-0.5, 0.5), (0.0, 0.0), (-3.0, -2.0), (2.0, 3.0)] {
        expressions.push(json!({"type":"clamp", "input":gradient, "min":min, "max":max}));
        expressions.push(json!({"type":"clamp", "input":nonlinear, "min":min, "max":max}));
    }
    let cached = json!({"type":"cache","input":nonlinear});
    expressions.push(json!({"type":"add","left":cached,"right":cached}));
    expressions.push(
        json!({"type":"cache","input":{"type":"clamp","input":gradient,"min":-0.5,"max":0.5}}),
    );
    expressions.push(json!({"type":"interpolated","cell_size_xz":4,"cell_size_y":8,"input":{"type":"add","left":cached,"right":cached}}));
    let spline = json!({"coordinate":gradient,"points":[
        {"location":-1.0,"value":-0.25,"derivative":0.75},
        {"location":0.0,"value":0.5,"derivative":-0.3},
        {"location":1.0,"value":1.25,"derivative":0.2}
    ]});
    let inner = json!({"coordinate":reverse,"points":[
        {"location":-1.0,"value":-0.5,"derivative":0.2},
        {"location":0.25,"value":0.25,"derivative":-0.1},
        {"location":0.75,"value":1.0,"derivative":0.0}
    ]});
    let nested = json!({"coordinate":gradient,"points":[
        {"location":-0.75,"value":inner,"derivative":0.3},
        {"location":0.5,"value":-0.2,"derivative":-0.4},
        {"location":1.25,"value":inner,"derivative":0.0}
    ]});
    expressions.push(json!({"type":"spline","spline":0.75}));
    expressions.push(json!({"type":"spline","spline":spline}));
    expressions.push(json!({"type":"spline","spline":nested}));
    expressions.push(json!({"type":"cache","input":{"type":"spline","spline":nested}}));
    expressions.push(json!({"type":"interpolated","cell_size_xz":4,"cell_size_y":8,"input":{"type":"spline","spline":nested}}));
    expressions
        .push(json!({"type":"min","left":{"type":"spline","spline":spline},"right":gradient}));
    expressions.push(json!({"type":"spline","spline":{"coordinate":gradient,"points":[{"location":0.0,"value":0.5,"derivative":0.0}]}}));
    expressions.push(json!({"type":"spline","spline":{"coordinate":gradient,"points":[{"location":0.0,"value":0.5,"derivative":0.0},{"location":0.0,"value":-0.5,"derivative":0.0},{"location":1.0,"value":0.25,"derivative":0.0}]}}));
    for derivative in [0.0, 1.0] {
        expressions.push(json!({"type":"spline","spline":{"coordinate":infinity,"points":[{"location":0.0,"value":0.5,"derivative":derivative}]}}));
    }
    expressions.push(json!({"type":"spline","spline":{"coordinate":{"type":"add","left":infinity,"right":negative_infinity},"points":[{"location":0.0,"value":0.5,"derivative":0.0}]}}));
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
    let mut ranges = 0;
    for line in output.lines() {
        let Some((kind, values)) = line.split_once(' ') else {
            continue;
        };
        if kind != "D" && kind != "R" {
            continue;
        }
        let parts: Vec<i32> = values
            .split_whitespace()
            .map(|p| p.parse().unwrap())
            .collect();
        if kind == "R" {
            let [index, min, max]: [i32; 3] = parts.try_into().unwrap();
            let range = compiled[index as usize].range();
            assert_eq!(
                (range.0.to_bits(), range.1.to_bits()),
                (min as u32, max as u32),
                "range of expression {index}: {}",
                expressions[index as usize]
            );
            ranges += 1;
            continue;
        }
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
    assert_eq!(ranges, expressions.len());
}
