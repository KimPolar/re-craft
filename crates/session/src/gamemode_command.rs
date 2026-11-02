//! Local-player subset of the Java `/gamemode` command.

use crate::GameMode;

fn mode(name: &str) -> Option<GameMode> {
    match name {
        "survival" => Some(GameMode::Survival),
        "creative" => Some(GameMode::Creative),
        "adventure" => Some(GameMode::Adventure),
        "spectator" => Some(GameMode::Spectator),
        _ => None,
    }
}

/// Commands may address the executing player implicitly or with `@s`.
pub fn parse(input: &str) -> Option<GameMode> {
    match input.split(' ').collect::<Vec<_>>().as_slice() {
        ["/gamemode", name] | ["/gamemode", name, "@s"] => mode(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_local_player_modes_and_rejects_unsupported_targets() {
        for (name, expected) in [
            ("survival", GameMode::Survival),
            ("creative", GameMode::Creative),
            ("adventure", GameMode::Adventure),
            ("spectator", GameMode::Spectator),
        ] {
            assert_eq!(parse(&format!("/gamemode {name}")), Some(expected));
            assert_eq!(parse(&format!("/gamemode {name} @s")), Some(expected));
        }
        for input in [
            "/gamemode",
            "/gamemode Creative",
            "/gamemode 1",
            "/gamemode creative @a",
            "/gamemode creative @s extra",
            "/gamemode  creative",
            "/gamemode creative ",
        ] {
            assert_eq!(parse(input), None, "{input}");
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn argument_and_flight_transition_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("GameModeOracle.java");
        let output = temp.path().join("output.json");
        let original = native_persistence::nbt::NamedTag {
            name: "".into(),
            value: native_persistence::nbt::Tag::Compound(vec![
                (
                    "playerGameType".into(),
                    native_persistence::nbt::Tag::Int(0),
                ),
                (
                    "abilities".into(),
                    native_persistence::nbt::Tag::Compound(vec![
                        (
                            "flySpeed".into(),
                            native_persistence::nbt::Tag::Float(0.075),
                        ),
                        (
                            "walkSpeed".into(),
                            native_persistence::nbt::Tag::Float(0.15),
                        ),
                        ("unknown".into(), native_persistence::nbt::Tag::Int(42)),
                    ]),
                ),
            ]),
        };
        let drafts = [
            (GameMode::Survival, false),
            (GameMode::Creative, false),
            (GameMode::Creative, true),
            (GameMode::Adventure, false),
            (GameMode::Spectator, true),
        ];
        for (index, (current, flying)) in drafts.into_iter().enumerate() {
            let draft = crate::player_data::draft_game_mode_transition(
                &original,
                GameMode::Survival,
                GameMode::Adventure,
                current,
                flying,
            )
            .unwrap();
            let path = temp.path().join(format!("output.json.draft{index}"));
            std::fs::write(path, native_persistence::nbt::write(&draft).unwrap()).unwrap();
        }
        std::fs::write(&source, include_str!("../tests/java/GameModeOracle.java")).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("GameModeOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        for ((current, flying), row) in drafts.into_iter().zip(data["drafts"].as_array().unwrap()) {
            assert_eq!(mode(row["mode"].as_str().unwrap()), Some(current));
            assert_eq!(row["previous"], "adventure");
            assert_eq!(row["flying"], flying);
            assert_eq!(
                row["mayfly"],
                matches!(current, GameMode::Creative | GameMode::Spectator)
            );
            assert_eq!(
                row["invulnerable"],
                matches!(current, GameMode::Creative | GameMode::Spectator)
            );
            assert_eq!(row["instabuild"], current == GameMode::Creative);
            assert_eq!(
                row["mayBuild"],
                matches!(current, GameMode::Survival | GameMode::Creative)
            );
            assert_eq!(row["flySpeed"].as_f64().unwrap() as f32, 0.075_f32);
            assert_eq!(row["walkSpeed"].as_f64().unwrap() as f32, 0.15_f32);
        }
        for row in data["arguments"].as_array().unwrap() {
            let value = row["input"].as_str().unwrap();
            assert_eq!(
                mode(value).is_some(),
                row["valid"].as_bool().unwrap(),
                "{value}"
            );
        }
        for row in data["abilities"].as_array().unwrap() {
            let target = mode(row["mode"].as_str().unwrap()).unwrap();
            assert_eq!(
                target.flight_after_change(row["initial"].as_bool().unwrap()),
                row["flying"].as_bool().unwrap(),
                "{row}"
            );
        }
        let full = [native_world::collision::Aabb {
            min: glam::Vec3::ZERO,
            max: glam::Vec3::ONE,
        }];
        let empty: [native_world::collision::Aabb; 0] = [];
        for row in data["ground"].as_array().unwrap() {
            let y = row["y"].as_f64().unwrap();
            let blocked = row["blocked"].as_bool().unwrap();
            let actual = crate::movement::ground_within_flight_range(
                glam::DVec3::new(0.5, y, 0.5),
                1.8,
                |pos| {
                    if pos.x == 0 && pos.z == 0 && (pos.y == 0 || (blocked && pos.y == 1)) {
                        &full[..]
                    } else {
                        &empty[..]
                    }
                },
            );
            assert_eq!(actual, row["near"].as_bool().unwrap(), "{row}");
        }
    }
}
