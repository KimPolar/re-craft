use native_world::{
    collision::{block_outline, block_shape, trapdoor_state},
    fluid::{FluidKind, FluidState},
    loaded::BlockState,
};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn all_trapdoor_shapes_and_fluids_match_java() -> Result<(), Box<dyn std::error::Error>> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    native_resources::packs::ResourceStack::open(&game)?;
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"]
        .as_array()
        .ok_or("missing libraries")?
    {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths)?;
    let source = temp.path().join("TrapdoorCollisionOracle.java");
    std::fs::write(&source, include_str!("java/TrapdoorCollisionOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source)
        .output()?;
    assert!(
        compiled.status.success(),
        "Java compilation: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("TrapdoorCollisionOracle")
        .output()?;
    assert!(
        output.status.success(),
        "Java oracle: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    let mut families = std::collections::BTreeSet::new();
    for line in text.lines().filter(|l| l.starts_with("S ")) {
        let row: Vec<_> = line.split_whitespace().collect();
        let state = BlockState {
            name: row[1].into(),
            properties: BTreeMap::from([
                ("facing".into(), row[2].into()),
                ("half".into(), row[3].into()),
                ("open".into(), row[4].into()),
                ("powered".into(), row[5].into()),
                ("waterlogged".into(), row[6].into()),
            ]),
        };
        let expected: Vec<f32> = row[7..13]
            .iter()
            .map(|v| v.parse())
            .collect::<std::result::Result<_, _>>()?;
        let shape = block_shape(&state).unwrap_or_else(|| panic!("missing collision {state:?}"));
        assert_eq!(shape.len(), 1, "unexpected box count");
        let b = shape[0];
        assert_eq!(
            [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z],
            expected.as_slice(),
            "{line}"
        );
        assert_eq!(block_outline(&state), Some(shape), "{line}");
        let flags = trapdoor_state(&state).ok_or("invalid trapdoor")?;
        assert_eq!(flags, (row[4].parse()?, row[6].parse()?), "{line}");
        let fluid = FluidState::from_block(&state).map_err(std::io::Error::other)?;
        let empty: bool = row[13].parse()?;
        assert_eq!(fluid.is_none(), empty, "{line}");
        if let Some(f) = fluid {
            assert_eq!(f.kind, FluidKind::Water);
            assert!(f.source);
        }
        families.insert(state.name);
        count += 1;
    }
    assert!(families.len() >= 10);
    assert_eq!(count, families.len() * 64);
    Ok(())
}
