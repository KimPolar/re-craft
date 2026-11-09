use native_world::{block_light::slab_stair_face_full, loaded::BlockState};
use std::collections::BTreeMap;

const DIRECTIONS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

#[test]
fn slab_faces_depend_on_type_and_validate_state() {
    let mut state = BlockState {
        name: "minecraft:stone_slab".into(),
        properties: BTreeMap::from([
            ("type".into(), "bottom".into()),
            ("waterlogged".into(), "false".into()),
        ]),
    };
    for (kind, expected) in [
        ("bottom", [true, false, false, false, false, false]),
        ("top", [false, true, false, false, false, false]),
        ("double", [true; 6]),
    ] {
        state.properties.insert("type".into(), kind.into());
        for (direction, full) in DIRECTIONS.into_iter().zip(expected) {
            assert_eq!(
                slab_stair_face_full(&state, direction),
                Some(full),
                "{state:?}"
            );
        }
    }
    assert_eq!(slab_stair_face_full(&state, [0, 0, 0]), None);
    state
        .properties
        .insert("waterlogged".into(), "invalid".into());
    assert_eq!(slab_stair_face_full(&state, [0, -1, 0]), None);
    state.name = "minecraft:invented_slab".into();
    state
        .properties
        .insert("waterlogged".into(), "false".into());
    assert_eq!(slab_stair_face_full(&state, [0, -1, 0]), None);
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn slab_stair_occlusion_faces_match_installed_java() -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::Value;
    use std::{path::PathBuf, process::Command};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    assert_eq!(manifest["id"], "26.3", "unexpected Minecraft version");
    let temp = tempfile::tempdir()?;
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_owned()];
    for library in manifest["libraries"]
        .as_array()
        .ok_or("missing libraries")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("MagmaSlabStairFacesOracle.java");
    std::fs::write(&source, include_str!("java/MagmaSlabStairFacesOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    assert!(
        compiled.status.success(),
        "javac failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let result = temp.path().join("faces.json");
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("MagmaSlabStairFacesOracle")
        .arg(&result)
        .output()?;
    assert!(
        output.status.success(),
        "Java failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(result)?)?;
    assert!(rows.len() > 1000, "missing slab and stair states");
    for row in &rows {
        let name = row["name"].as_str().ok_or("missing name")?;
        let properties = row["properties"]
            .as_object()
            .ok_or("missing properties")?
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.clone(),
                    value.as_str().ok_or("invalid property")?.to_owned(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, &str>>()?;
        let state = BlockState {
            name: name.to_owned(),
            properties,
        };
        let faces = row["faces"].as_array().ok_or("missing faces")?;
        assert_eq!(faces.len(), DIRECTIONS.len(), "invalid faces");
        for (direction, expected) in DIRECTIONS.into_iter().zip(faces) {
            assert_eq!(
                slab_stair_face_full(&state, direction),
                expected.as_bool(),
                "{state:?} {direction:?}"
            );
        }
    }
    eprintln!(
        "Compared {} slab and stair states across six faces",
        rows.len()
    );
    Ok(())
}
