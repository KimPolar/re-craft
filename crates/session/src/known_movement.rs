use glam::DVec3;

/// Models the local client's position reports and the server's last accepted
/// movement, assuming each report arrives in its originating client tick.
pub(crate) struct ClientMovement {
    last_reported: DVec3,
    server_position: DVec3,
    reminder: i32,
}

impl ClientMovement {
    pub(crate) fn new(server_position: DVec3) -> Self {
        // LocalPlayer's last-sent coordinates start at zero, independently of
        // the server player's spawn position.
        Self {
            last_reported: DVec3::ZERO,
            server_position,
            reminder: 0,
        }
    }

    pub(crate) fn advance(&mut self, position: DVec3) -> DVec3 {
        self.reminder = self.reminder.wrapping_add(1);
        let delta = position - self.last_reported;
        if delta.length_squared() > 0.0002_f64.powi(2) || self.reminder >= 20 {
            self.last_reported = position;
            self.reminder = 0;
            let movement = position - self.server_position;
            self.server_position = position;
            movement
        } else {
            // Rotation/status-only packets report no movement; absent packets
            // are reset by the client's end-of-tick packet.
            DVec3::ZERO
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_small_changes_and_clears_without_a_position_report() {
        let mut tracker = ClientMovement::new(DVec3::ZERO);
        assert_eq!(tracker.advance(DVec3::new(0.0001, 0.0, 0.0)), DVec3::ZERO);
        assert_eq!(tracker.advance(DVec3::new(0.0002, 0.0, 0.0)), DVec3::ZERO);
        assert_eq!(
            tracker.advance(DVec3::new(0.0003, 0.0, 0.0)),
            DVec3::new(0.0003, 0.0, 0.0)
        );
        assert_eq!(tracker.advance(DVec3::new(0.0003, 0.0, 0.0)), DVec3::ZERO);
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn position_reporting_matches_java_local_player() {
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let source = temp.path().join("KnownMovementOracle.java");
        let output = temp.path().join("movement.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/KnownMovementOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("KnownMovementOracle")
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
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 6);
        for (scenario, row) in rows.iter().enumerate() {
            let base = row["base"].as_f64().unwrap();
            let mut tracker = ClientMovement::new(DVec3::new(
                base,
                if scenario == 0 { 0.0 } else { 64.0 },
                -base,
            ));
            for (tick, step) in row["steps"].as_array().unwrap().iter().enumerate() {
                let values = |name: &str| -> [f64; 3] {
                    let a = step[name].as_array().unwrap();
                    std::array::from_fn(|i| f64::from_bits(a[i].as_i64().unwrap() as u64))
                };
                let actual = tracker.advance(DVec3::from_array(values("position")));
                assert_eq!(
                    actual.to_array().map(f64::to_bits),
                    values("known").map(f64::to_bits),
                    "scenario {scenario}, tick {tick}"
                );
                assert_eq!(
                    tracker.reminder == 0,
                    step["reported"].as_bool().unwrap(),
                    "scenario {scenario}, tick {tick}"
                );
            }
        }
    }
}
