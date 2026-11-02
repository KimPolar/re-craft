//! Client-side particles from dropped items entering water.
use std::collections::HashMap;

use glam::DVec3;
use native_core::random::LegacyRandom;
use native_session::{ItemWaterEntry, loaded::DrownParticleSpawn};

const LIMIT: usize = 4096;

fn generate(
    event: ItemWaterEntry,
    random: &mut LegacyRandom,
) -> (Vec<DrownParticleSpawn>, Vec<DrownParticleSpawn>) {
    // Client entities have a distinct random source from the server entity.
    random.next_float();
    random.next_float();
    let width = f64::from(0.25_f32);
    let y = f64::from((event.position.y.floor() as i32) as f32 + 1.0_f32);
    let mut bubbles = Vec::with_capacity(6);
    let mut splashes = Vec::with_capacity(6);
    for _ in 0..6 {
        let x = (random.next_double() * 2.0 - 1.0) * width;
        let z = (random.next_double() * 2.0 - 1.0) * width;
        let dy = event.velocity.y - random.next_double() * f64::from(0.2_f32);
        bubbles.push(DrownParticleSpawn {
            tick: event.tick,
            position: DVec3::new(event.position.x + x, y, event.position.z + z),
            velocity: DVec3::new(event.velocity.x, dy, event.velocity.z),
        });
    }
    for _ in 0..6 {
        let x = (random.next_double() * 2.0 - 1.0) * width;
        let z = (random.next_double() * 2.0 - 1.0) * width;
        splashes.push(DrownParticleSpawn {
            tick: event.tick,
            position: DVec3::new(event.position.x + x, y, event.position.z + z),
            velocity: event.velocity,
        });
    }
    (bubbles, splashes)
}

pub(crate) struct WaterEntryEmitter {
    seed: LegacyRandom,
    entities: HashMap<u32, (LegacyRandom, u64)>,
}

impl Default for WaterEntryEmitter {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos() as i64);
        Self {
            seed: LegacyRandom::new(seed),
            entities: HashMap::new(),
        }
    }
}

impl WaterEntryEmitter {
    pub fn emit(
        &mut self,
        event: ItemWaterEntry,
    ) -> (Vec<DrownParticleSpawn>, Vec<DrownParticleSpawn>) {
        if !self.entities.contains_key(&event.entity_id) && self.entities.len() >= LIMIT {
            if let Some(oldest) = self
                .entities
                .iter()
                .min_by_key(|(id, (_, tick))| (*tick, **id))
                .map(|(id, _)| *id)
            {
                self.entities.remove(&oldest);
            }
        }
        let (random, tick) = self
            .entities
            .entry(event.entity_id)
            .or_insert_with(|| (LegacyRandom::new(self.seed.next_long()), event.tick));
        *tick = event.tick;
        generate(event, random)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_entries_keep_independent_entity_random_streams() {
        let mut emitter = WaterEntryEmitter {
            seed: LegacyRandom::new(19),
            entities: HashMap::new(),
        };
        let event = ItemWaterEntry {
            tick: 4,
            entity_id: 1,
            position: DVec3::new(1.25, 4.5, -3.75),
            velocity: DVec3::new(0.3, -0.2, -0.4),
            source_uuid: None,
        };
        let mut seeds = LegacyRandom::new(19);
        let mut first = LegacyRandom::new(seeds.next_long());
        let mut second = LegacyRandom::new(seeds.next_long());
        assert_eq!(emitter.emit(event), generate(event, &mut first));
        let other = ItemWaterEntry {
            entity_id: 2,
            ..event
        };
        assert_eq!(emitter.emit(other), generate(other, &mut second));
        let later = ItemWaterEntry { tick: 9, ..event };
        assert_eq!(emitter.emit(later), generate(later, &mut first));
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn item_water_particles_match_java() {
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
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ItemWaterSplashOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../../session/tests/java/ItemWaterSplashOracle.java"),
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
                .arg("ItemWaterSplashOracle")
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
        assert_eq!(rows.len(), 320);
        for row in rows {
            let event = ItemWaterEntry {
                tick: 3,
                entity_id: 5,
                position: DVec3::new(1.25, 4.5, -3.75),
                velocity: DVec3::new(0.3, row["y"].as_f64().unwrap(), -0.4),
                source_uuid: None,
            };
            let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let (bubbles, splashes) = generate(event, &mut random);
            let expected = row["particles"].as_array().unwrap();
            assert_eq!(expected.len(), 12);
            for (index, (particle, expected)) in
                bubbles.iter().chain(&splashes).zip(expected).enumerate()
            {
                assert_eq!(expected["bubble"].as_bool(), Some(index < 6));
                let actual = [
                    particle.position.x,
                    particle.position.y,
                    particle.position.z,
                    particle.velocity.x,
                    particle.velocity.y,
                    particle.velocity.z,
                ];
                for (actual, expected) in
                    actual.into_iter().zip(expected["data"].as_array().unwrap())
                {
                    assert_eq!(actual.to_bits(), expected.as_i64().unwrap() as u64, "{row}");
                }
            }
            assert_eq!(random.next_int(), row["next"].as_i64().unwrap() as i32);
        }
    }
}
