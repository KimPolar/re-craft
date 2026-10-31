//! Atmospheric rain fog in Java 26.3's frame-driven renderer.

use glam::DVec3;
use native_session::WorldSnapshot;

/// Return no sample when the loaded snapshot lacks the camera's biome or sky light.
pub fn inputs(snapshot: &WorldSnapshot, camera: DVec3, partial: f32) -> Option<(f32, u8, bool)> {
    if !camera.is_finite() {
        return None;
    }
    let floor = camera.floor();
    if floor.min_element() < f64::from(i32::MIN + 16)
        || floor.max_element() > f64::from(i32::MAX - 16)
    {
        return None;
    }
    let position = floor.as_ivec3();
    let terrain = snapshot.weather_terrain.as_ref()?;
    let world = snapshot.world.loaded()?;
    let biome = world.biome_at_block_in_bounds(
        position.x,
        position.y,
        position.z,
        terrain.min_y,
        terrain.max_y,
    )?;
    let precipitation = terrain.climates.get(biome)?.has_precipitation;
    let light = world.light(position.x, position.y, position.z, true)?;
    Some((snapshot.weather?.rain_at(partial), light, precipitation))
}

#[derive(Default)]
pub struct RainFog {
    multiplier: f32,
}

impl RainFog {
    pub fn update(&mut self, rain: f32, sky_light: u8, has_precipitation: bool, delta_ticks: f32) {
        let light = ((f32::from(sky_light) - 8.0) / 7.0).clamp(0.0, 1.0);
        let target = rain * light * if has_precipitation { 1.0 } else { 0.5 };
        self.multiplier += (target - self.multiplier) * delta_ticks * 0.2;
    }

    pub fn apply(&self, range: [f32; 2]) -> [f32; 2] {
        [
            range[0] + -160.0 * self.multiplier,
            range[1].min(96.0).max(range[1] + -256.0 * self.multiplier),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_sample_requires_loaded_biome_and_sky_light() {
        use native_resources::biome_climate::{BiomeClimate, TemperatureModifier};
        use native_session::{
            GameSession, LocalSession, SessionWorld,
            weather::{WeatherTerrain, WeatherVisual},
        };
        use native_world::loaded::{BIOME_COUNT, Chunk, ChunkPos, LIGHT_BYTES, Section, World};
        use std::{collections::BTreeMap, sync::Arc};
        let mut world = World::new("minecraft:overworld");
        world.set_biome_world_seed(Some(7));
        let biome = world.intern_biome("minecraft:desert".into());
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:full".into()),
                block_light_correct: true,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: None,
                        biomes: Some(Box::new([biome; BIOME_COUNT])),
                        block_light: None,
                        sky_light: Some(Box::new([0xcc; LIGHT_BYTES])),
                    }),
                )]),
            }),
        );
        let mut snapshot = (*LocalSession::demo().snapshot()).clone();
        snapshot.world = SessionWorld::Loaded(Arc::new(world));
        snapshot.weather = Some(WeatherVisual {
            previous_rain: 0.4,
            rain: 0.8,
            ..Default::default()
        });
        snapshot.weather_terrain = Some(WeatherTerrain {
            heightmaps: Arc::new(BTreeMap::new()),
            climates: Arc::new(BTreeMap::from([(
                "minecraft:desert".into(),
                BiomeClimate {
                    has_precipitation: false,
                    temperature: 2.0,
                    modifier: TemperatureModifier::None,
                },
            )])),
            sea_level: Some(63),
            min_y: 0,
            max_y: 16,
        });
        assert_eq!(
            inputs(&snapshot, DVec3::new(8.5, 8.5, 8.5), 0.5),
            Some((0.6, 12, false))
        );
        assert_eq!(inputs(&snapshot, DVec3::new(100.0, 8.5, 8.5), 0.5), None);
        assert_eq!(inputs(&snapshot, DVec3::new(f64::NAN, 8.5, 8.5), 0.5), None);
        assert_eq!(inputs(&snapshot, DVec3::new(8.5, 40.0, 8.5), 0.5), None);
    }

    #[test]
    fn rain_fog_tracks_sky_light_biome_and_fades_after_rain() {
        let mut fog = RainFog::default();
        assert_eq!(fog.apply([0.0, 1024.0]), [0.0, 1024.0]);
        fog.update(1.0, 8, true, 1.0);
        assert_eq!(fog.apply([0.0, 1024.0]), [0.0, 1024.0]);
        fog.update(1.0, 15, true, 1.0);
        assert_eq!(fog.apply([0.0, 1024.0]), [-32.0, 972.8]);
        fog.update(0.0, 15, true, 1.0);
        assert!((fog.multiplier - 0.16).abs() < 0.000_001);
        let mut dry_biome = RainFog::default();
        dry_biome.update(1.0, 15, false, 1.0);
        assert_eq!(dry_biome.multiplier, 0.1);
        assert_eq!(
            RainFog { multiplier: 1.0 }.apply([10.0, 120.0]),
            [-150.0, 96.0]
        );
        assert_eq!(
            RainFog { multiplier: 1.0 }.apply([10.0, 50.0]),
            [-150.0, 50.0]
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn rain_fog_matches_java_frame_updates() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let source = temp.path().join("RainFogOracle.java");
        let output = temp.path().join("fog.txt");
        std::fs::write(&source, include_str!("../tests/java/RainFogOracle.java")).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("RainFogOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "oracle failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let text = std::fs::read_to_string(output).unwrap();
        let mut fog = RainFog::default();
        let mut count = 0;
        for (i, line) in text.lines().enumerate() {
            if i % 100 == 0 {
                fog = RainFog::default();
            }
            let n: Vec<i32> = line
                .split_whitespace()
                .map(|v| v.parse().unwrap())
                .collect();
            assert_eq!(n.len(), 5);
            fog.update(
                f32::from_bits(n[0] as u32),
                n[1] as u8,
                n[2] != 0,
                f32::from_bits(n[3] as u32),
            );
            assert_eq!(fog.multiplier.to_bits(), n[4] as u32, "case {i}: {line}");
            count += 1;
        }
        assert_eq!(count, 5000);
    }
}
