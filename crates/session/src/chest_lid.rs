//! Fixed-tick chest lid progress, independent of rendering frame rate.

use crate::inventory::InventoryMenu;
use glam::IVec3;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChestLidVisual {
    pub position: IVec3,
    pub previous: f32,
    pub current: f32,
}

impl ChestLidVisual {
    pub fn openness(self, partial: f32) -> f32 {
        self.previous + partial.clamp(0.0, 1.0) * (self.current - self.previous)
    }
    pub fn angle(self, partial: f32) -> f32 {
        let remaining = 1.0 - self.openness(partial);
        -(1.0 - remaining * remaining * remaining) * std::f32::consts::FRAC_PI_2
    }
}

#[derive(Default)]
pub(crate) struct ChestLids {
    states: BTreeMap<(i32, i32, i32), ChestLidVisual>,
    snapshot: Arc<[ChestLidVisual]>,
}

impl ChestLids {
    pub fn tick(&mut self, menu: Option<InventoryMenu>) {
        let open = match menu {
            Some(InventoryMenu::Chest { position, other }) => [Some(position), other],
            _ => [None, None],
        };
        for position in open.into_iter().flatten() {
            self.states
                .entry((position.x, position.y, position.z))
                .or_insert(ChestLidVisual {
                    position,
                    previous: 0.0,
                    current: 0.0,
                });
        }
        for lid in self.states.values_mut() {
            lid.previous = lid.current;
            if open.contains(&Some(lid.position)) {
                if lid.current < 1.0 {
                    lid.current = (lid.current + 0.1).min(1.0);
                }
            } else if lid.current > 0.0 {
                lid.current = (lid.current - 0.1).max(0.0);
            }
        }
        self.states
            .retain(|_, lid| lid.previous != 0.0 || lid.current != 0.0);
        if !self
            .states
            .values()
            .copied()
            .eq(self.snapshot.iter().copied())
        {
            self.snapshot = self.states.values().copied().collect();
        }
    }
    pub fn snapshot(&self) -> Arc<[ChestLidVisual]> {
        Arc::clone(&self.snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn double_lids_share_tick_state_and_remove_after_closing() {
        let mut lids = ChestLids::default();
        let right = IVec3::ZERO;
        let left = IVec3::X;
        for _ in 0..5 {
            lids.tick(Some(InventoryMenu::Chest {
                position: right,
                other: Some(left),
            }));
        }
        let open = lids.snapshot();
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].current, 0.5);
        assert_eq!(open[1].current, 0.5);
        lids.tick(None);
        assert_eq!(open[0].current, 0.5);
        assert_eq!(lids.snapshot()[0].previous, 0.5);
        for _ in 0..12 {
            lids.tick(None);
        }
        assert!(lids.snapshot().is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn lid_ticks_and_interpolation_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ChestLidTickOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/ChestLidTickOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java lid oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("ChestLidTickOracle")
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let mut lids = ChestLids::default();
        for (tick, row) in rows.iter().enumerate() {
            let open = tick < 12 || (17..23).contains(&tick);
            lids.tick(open.then_some(InventoryMenu::Chest {
                position: IVec3::ZERO,
                other: None,
            }));
            let lid = lids.snapshot().first().copied().unwrap_or(ChestLidVisual {
                position: IVec3::ZERO,
                previous: 0.0,
                current: 0.0,
            });
            for (sample, partial) in row
                .as_array()
                .unwrap()
                .iter()
                .zip([0.0_f32, 0.25, 0.5, 1.0])
            {
                for (actual, expected) in [lid.openness(partial), lid.angle(partial)]
                    .into_iter()
                    .zip(sample.as_array().unwrap())
                {
                    assert!(
                        (f64::from(actual) - expected.as_f64().unwrap()).abs() < 0.000001,
                        "tick {tick} partial {partial}"
                    );
                }
            }
        }
    }
}
