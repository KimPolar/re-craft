//! Compare lava spread delays and RNG consumption with installed Java 26.3.

use native_core::random::LegacyRandom;
use native_world::{
    fluid::{FluidKind, FluidState},
    fluid_sim::{self, Settings},
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn lava_spread_delay_and_random_consumption_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("LavaDelayOracle.java");
    let output = temp.path().join("delays.json");
    std::fs::write(&source, include_str!("java/LavaDelayOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("LavaDelayOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "lava delay oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 2 * 3 * 17 * 17 * 7);
    let state = |level: i64| {
        (level >= 0).then(|| FluidState {
            kind: FluidKind::Lava,
            amount: if level == 0 || level >= 8 {
                8
            } else {
                (8 - level) as u8
            },
            source: level == 0,
            falling: level >= 8,
        })
    };
    for row in rows {
        let settings = Settings {
            fast_lava: row[0].as_bool().unwrap(),
            ..Default::default()
        };
        let mut random = LegacyRandom::new(row[4].as_i64().unwrap());
        let actual = fluid_sim::spread_delay(
            settings,
            FluidKind::Lava,
            state(row[2].as_i64().unwrap()),
            state(row[3].as_i64().unwrap()),
            row[1].as_i64() == Some(1),
            || random.next_int_bound(4).unwrap() as u32,
        );
        assert_eq!(actual, row[5].as_u64().unwrap(), "{row}");
        assert_eq!(random.next_int(), row[6].as_i64().unwrap() as i32, "{row}");
    }
}
