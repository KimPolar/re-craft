use glam::IVec3;
use native_world::{door, loaded::BlockState};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

fn door_state(
    name: &str,
    facing: &str,
    half: &str,
    hinge: &str,
    open: &str,
    powered: &str,
) -> BlockState {
    BlockState {
        name: name.into(),
        properties: BTreeMap::from([
            ("facing".into(), facing.into()),
            ("half".into(), half.into()),
            ("hinge".into(), hinge.into()),
            ("open".into(), open.into()),
            ("powered".into(), powered.into()),
        ]),
    }
}
fn block(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: BTreeMap::new(),
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn door_neighbor_updates_match_java() -> Result<(), Box<dyn std::error::Error>> {
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
    let source = temp.path().join("DoorUpdateOracle.java");
    std::fs::write(&source, include_str!("java/DoorUpdateOracle.java"))?;
    let result = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source)
        .output()?;
    assert!(
        result.status.success(),
        "Java compile: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("DoorUpdateOracle")
        .output()?;
    assert!(
        result.status.success(),
        "Java oracle: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let text = String::from_utf8(result.stdout)?;
    let mut count = 0;
    for line in text.lines().filter(|l| l.starts_with("D ")) {
        let row: Vec<_> = line.split_whitespace().collect();
        let state = door_state(row[1], row[2], row[3], row[4], row[5], row[6]);
        let direction = match row[7] {
            "down" => IVec3::NEG_Y,
            "up" => IVec3::Y,
            "north" => IVec3::NEG_Z,
            "south" => IVec3::Z,
            "west" => IVec3::NEG_X,
            "east" => IVec3::X,
            _ => panic!("bad direction"),
        };
        let opposite = if row[3] == "lower" { "upper" } else { "lower" };
        let neighbor = match row[8] {
            "0" => block("minecraft:air"),
            "1" => state.clone(),
            "2" => {
                let mut n = state.clone();
                n.properties.insert("half".into(), opposite.into());
                n
            }
            "3" => door_state(
                "minecraft:birch_door",
                "east",
                opposite,
                "left",
                "true",
                "false",
            ),
            "4" => block("minecraft:stone"),
            _ => panic!("bad neighbor"),
        };
        let actual = door::update_shape(&state, direction, &neighbor, Some(row[9].parse()?))
            .expect("known state");
        let expected = if row[10] == "minecraft:air" {
            block("minecraft:air")
        } else {
            door_state(row[10], row[11], row[12], row[13], row[14], row[15])
        };
        assert_eq!(actual, expected, "{line}");
        count += 1;
    }
    assert_eq!(count, 64 * 6 * 5 * 2);
    Ok(())
}
