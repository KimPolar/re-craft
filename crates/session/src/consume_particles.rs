//! 26.3 item-use particle spawn coordinates. Rendering and lifetime are client-side.

use std::sync::Arc;

use glam::DVec3;

use crate::{inventory::ItemStack, physics::java_sin};

/// Handle a client-side equipment-break event using the item the client saw
/// before its equipment update. The sound shares the normal sound
/// queue; particles use the same item renderer as consumption particles.
#[allow(clippy::too_many_arguments)]
pub(crate) fn equipment_break(
    item: &Arc<ItemStack>,
    silent: bool,
    position: DVec3,
    eye: DVec3,
    yaw: f32,
    pitch: f32,
    tick: u64,
    random: &mut native_core::random::LegacyRandom,
    sounds: &mut std::collections::VecDeque<crate::ConsumeSound>,
    particles: &mut std::collections::VecDeque<ConsumeParticleSpawn>,
) {
    let sound: Option<std::borrow::Cow<'static, str>> =
        if item.components().get("!minecraft:break_sound").is_some() {
            None
        } else {
            match item.components().get("minecraft:break_sound") {
                None => Some(
                    (if item.id() == "minecraft:shield" {
                        "item.shield.break"
                    } else {
                        "entity.item.break"
                    })
                    .into(),
                ),
                Some(value) => break_sound_id(value).map(std::borrow::Cow::Owned),
            }
        };
    if !silent && let Some(event) = sound {
        let pitch = 0.8_f32 + random.next_float() * 0.4_f32;
        crate::queue_player_sound(sounds, position, event, 0.8, pitch);
    }
    for _ in 0..5 {
        let samples = [
            random.next_float(),
            random.next_float(),
            random.next_float(),
            random.next_float(),
        ];
        crate::push_consume_particle(particles, item, samples, eye, yaw, pitch, tick);
    }
}

/// Decode the registered sound ID or inline SoundEvent codec representation.
pub(crate) fn break_sound_id(value: &native_persistence::nbt::Tag) -> Option<String> {
    use native_persistence::nbt::Tag;
    let sound = match value {
        Tag::String(sound) => sound,
        Tag::Compound(fields) => {
            let mut matching = fields.iter().filter(|(name, _)| name.matches("sound_id"));
            let (_, Tag::String(sound)) = matching.next()? else {
                return None;
            };
            if matching.next().is_some() {
                return None;
            }
            sound
        }
        _ => return None,
    };
    let text = String::from_utf16(&sound.0).ok()?;
    let id = native_resources::packs::ResourceId::parse(&text).ok()?;
    Some(format!("{}:{}", id.namespace, id.path))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConsumeParticleSpawn {
    pub item: Arc<ItemStack>,
    pub tick: u64,
    pub position: DVec3,
    pub velocity: DVec3,
}

/// The four floats consumed by LivingEntity.spawnItemParticles, in order.
pub(crate) fn particle_vectors(
    samples: [f32; 4],
    eye: DVec3,
    yaw: f32,
    pitch: f32,
) -> (DVec3, DVec3) {
    let rotate_x = -pitch * (std::f32::consts::PI / 180.0);
    let rotate_y = -yaw * (std::f32::consts::PI / 180.0);
    let sx = f64::from(java_sin(f64::from(rotate_x), false));
    let cx = f64::from(java_sin(f64::from(rotate_x), true));
    let sy = f64::from(java_sin(f64::from(rotate_y), false));
    let cy = f64::from(java_sin(f64::from(rotate_y), true));
    let rotate = |v: DVec3| {
        let y = v.y * cx + v.z * sx;
        let z = v.z * cx - v.y * sx;
        DVec3::new(v.x * cy + z * sy, y, z * cy - v.x * sy)
    };
    let velocity = rotate(DVec3::new(
        (f64::from(samples[0]) - 0.5) * 0.1,
        f64::from(samples[1]) * 0.1 + 0.1,
        0.0,
    ));
    let velocity = DVec3::new(velocity.x, velocity.y + 0.05, velocity.z);
    let offset = rotate(DVec3::new(
        (f64::from(samples[3]) - 0.5) * 0.3,
        f64::from(-samples[2]) * 0.6 - 0.3,
        0.6,
    ));
    (eye + offset, velocity)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use std::{path::PathBuf, process::Command};

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn equipment_break_feedback_matches_java() {
        use native_persistence::nbt::Tag;
        use std::collections::VecDeque;
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
        let source = temp.path().join("EquipmentBreakOracle.java");
        let output = temp.path().join("break.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/EquipmentBreakOracle.java"),
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
                .arg("EquipmentBreakOracle")
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
        assert_eq!(rows.len(), 576);
        for row in rows {
            let mut components = Vec::new();
            match row["variant"].as_i64().unwrap() {
                0 => {}
                1 => components.push(("!minecraft:break_sound".into(), Tag::Compound(vec![]))),
                2 => components.push((
                    "minecraft:break_sound".into(),
                    Tag::String("block.anvil.break".into()),
                )),
                3 => components.push((
                    "minecraft:break_sound".into(),
                    Tag::Compound(vec![
                        ("sound_id".into(), Tag::String("example:hammer".into())),
                        ("range".into(), Tag::Float(8.0)),
                    ]),
                )),
                _ => panic!(),
            }
            let item = Arc::new(
                ItemStack::new(
                    "minecraft:iron_helmet".into(),
                    1,
                    1,
                    Tag::Compound(components),
                )
                .unwrap(),
            );
            let mut random = native_core::random::LegacyRandom::new(row["seed"].as_i64().unwrap());
            let mut sounds = VecDeque::new();
            let mut particles = VecDeque::new();
            equipment_break(
                &item,
                row["silent"].as_bool().unwrap(),
                DVec3::new(12.25, 64.0, -30.75),
                DVec3::new(12.25, 65.62, -30.75),
                row["yaw"].as_f64().unwrap() as f32,
                row["pitch"].as_f64().unwrap() as f32,
                1,
                &mut random,
                &mut sounds,
                &mut particles,
            );
            let java_sounds = row["sounds"].as_array().unwrap();
            assert_eq!(sounds.len(), java_sounds.len(), "{row}");
            for (sound, expected) in sounds.iter().zip(java_sounds) {
                let actual = native_resources::packs::ResourceId::parse(&sound.info.event).unwrap();
                assert_eq!(
                    format!("{}:{}", actual.namespace, actual.path),
                    expected["event"].as_str().unwrap()
                );
                assert_eq!(
                    sound.info.volume.to_bits(),
                    expected["volume"].as_i64().unwrap() as u32
                );
                assert_eq!(
                    sound.info.pitch.to_bits(),
                    expected["pitch"].as_i64().unwrap() as u32
                );
            }
            for (particle, expected) in particles.iter().zip(row["particles"].as_array().unwrap()) {
                for (actual, value) in particle
                    .position
                    .to_array()
                    .into_iter()
                    .chain(particle.velocity.to_array())
                    .zip(expected.as_array().unwrap())
                {
                    assert_eq!(actual.to_bits(), value.as_i64().unwrap() as u64, "{row}");
                }
            }
            assert_eq!(particles.len(), 5);
            assert_eq!(
                random.next_float().to_bits(),
                row["next_bits"].as_i64().unwrap() as u32,
                "{row}"
            );
        }
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn particle_spawn_vectors_match_java() {
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
        let source = temp.path().join("ConsumeParticleSpawnOracle.java");
        let output = temp.path().join("particles.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/ConsumeParticleSpawnOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("ConsumeParticleSpawnOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java particle oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 200);
        for row in rows {
            let mut rng = native_core::random::LegacyRandom::new(row["seed"].as_i64().unwrap());
            for expected in row["particles"].as_array().unwrap() {
                let samples = [
                    rng.next_float(),
                    rng.next_float(),
                    rng.next_float(),
                    rng.next_float(),
                ];
                let (pos, velocity) = particle_vectors(
                    samples,
                    DVec3::new(12.25, 65.62, -30.75),
                    row["yaw"].as_f64().unwrap() as f32,
                    row["pitch"].as_f64().unwrap() as f32,
                );
                for (actual, value) in pos
                    .to_array()
                    .into_iter()
                    .chain(velocity.to_array())
                    .zip(expected.as_array().unwrap())
                {
                    assert_eq!(actual.to_bits(), value.as_i64().unwrap() as u64, "{row}");
                }
            }
            assert_eq!(
                rng.next_float().to_bits(),
                row["next_bits"].as_i64().unwrap() as i32 as u32,
                "{row}"
            );
        }
    }
}
