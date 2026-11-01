//! Client-side soul particles from enchantment tick effects.

use glam::DVec3;
use native_core::random::LegacyRandom;
use native_session::{SessionWorld, loaded::SoulParticleSpawn};
use native_world::{collision::Aabb, movement::BoundingBox};
use std::collections::{BTreeMap, HashMap, VecDeque};

#[derive(Clone, Debug)]
pub(crate) struct Soul {
    pub previous: DVec3,
    pub position: DVec3,
    velocity: DVec3,
    bounds: BoundingBox,
    pub size: f32,
    age: i32,
    lifetime: i32,
    stopped: bool,
    on_ground: bool,
}

impl Soul {
    fn new(spawn: SoulParticleSpawn, random: &mut LegacyRandom) -> Self {
        let _initial_lifetime = (4.0_f32 / (random.next_float() * 0.9_f32 + 0.1_f32)) as i32;
        let mut component =
            |input: f64| input + f64::from((random.next_float() * 2.0_f32 - 1.0_f32) * 0.4_f32);
        let base = DVec3::new(
            component(spawn.velocity.x),
            component(spawn.velocity.y),
            component(spawn.velocity.z),
        );
        let magnitude = f64::from((random.next_float() + random.next_float() + 1.0_f32) * 0.15_f32);
        let length = (base.x * base.x + base.y * base.y + base.z * base.z).sqrt();
        let initial = DVec3::new(
            base.x / length * magnitude * f64::from(0.4_f32),
            base.y / length * magnitude * f64::from(0.4_f32) + f64::from(0.1_f32),
            base.z / length * magnitude * f64::from(0.4_f32),
        );
        let size = (0.1_f32 * (random.next_float() * 0.5_f32 + 0.5_f32) * 2.0_f32) * 1.5_f32;
        let mut jitter = || f64::from((random.next_float() - random.next_float()) * 0.05_f32);
        let position = spawn.position + DVec3::new(jitter(), jitter(), jitter());
        let lifetime = (8.0_f64 / (f64::from(random.next_float()) * 0.8 + 0.2)) as i32 + 4;
        let old_half = f64::from(0.1_f32);
        let width = f64::from(0.2_f32 * 1.5_f32);
        let min_x = ((spawn.position.x - old_half) + (spawn.position.x + old_half) - width) / 2.0;
        let min_z = ((spawn.position.z - old_half) + (spawn.position.z + old_half) - width) / 2.0;
        Self {
            previous: spawn.position,
            position,
            velocity: initial * f64::from(0.01_f32) + spawn.velocity,
            bounds: BoundingBox {
                min: DVec3::new(min_x, spawn.position.y, min_z),
                max: DVec3::new(min_x + width, spawn.position.y + width, min_z + width),
            },
            size,
            age: 0,
            lifetime: lifetime.max(1),
            stopped: false,
            on_ground: false,
        }
    }

    fn tick_with(&mut self, mut resolve: impl FnMut(BoundingBox, DVec3) -> DVec3) -> bool {
        self.previous = self.position;
        let age = self.age;
        self.age += 1;
        if age >= self.lifetime {
            return false;
        }
        if !self.stopped {
            let requested = self.velocity;
            let actual = resolve(self.bounds, requested);
            if actual != DVec3::ZERO {
                self.bounds = self.bounds.moved(actual);
                self.position = DVec3::new(
                    (self.bounds.min.x + self.bounds.max.x) / 2.0,
                    self.bounds.min.y,
                    (self.bounds.min.z + self.bounds.max.z) / 2.0,
                );
            }
            if requested.y.abs() >= f64::from(1.0e-5_f32) && actual.y.abs() < f64::from(1.0e-5_f32)
            {
                self.stopped = true;
            }
            self.on_ground = requested.y != actual.y && requested.y < 0.0;
            if requested.x != actual.x {
                self.velocity.x = 0.0;
            }
            if requested.z != actual.z {
                self.velocity.z = 0.0;
            }
        }
        self.velocity *= f64::from(0.96_f32);
        if self.on_ground {
            self.velocity.x *= f64::from(0.7_f32);
            self.velocity.z *= f64::from(0.7_f32);
        }
        true
    }

    pub fn frame(&self, count: usize) -> usize {
        ((self.age as usize) * count.saturating_sub(1) / self.lifetime as usize)
            .min(count.saturating_sub(1))
    }
}

pub(crate) struct SoulSystem {
    particles: VecDeque<Soul>,
    pending: BTreeMap<u64, Vec<SoulParticleSpawn>>,
    pending_count: usize,
    tick: Option<u64>,
    random: LegacyRandom,
    shapes: HashMap<u32, Option<Vec<Aabb>>>,
}
impl Default for SoulSystem {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |v| v.as_nanos() as i64);
        Self {
            particles: VecDeque::new(),
            pending: BTreeMap::new(),
            pending_count: 0,
            tick: None,
            random: LegacyRandom::new(seed),
            shapes: HashMap::new(),
        }
    }
}
impl SoulSystem {
    pub fn enqueue(&mut self, events: Vec<SoulParticleSpawn>) {
        for event in events {
            if self.pending_count >= 4096 {
                break;
            }
            self.pending.entry(event.tick).or_default().push(event);
            self.pending_count += 1;
        }
    }
    pub fn update(&mut self, target: u64, world: &SessionWorld) {
        let first = self
            .pending
            .first_key_value()
            .map_or(target, |(&tick, _)| tick.saturating_sub(1).min(target));
        let mut tick = self.tick.unwrap_or(first);
        if target < tick {
            self.particles.clear();
            self.pending.clear();
            self.pending_count = 0;
            self.shapes.clear();
            tick = target;
        }
        if target.saturating_sub(tick) > 200 {
            self.particles.clear();
            tick = target - 200;
            self.pending.retain(|&t, _| t > tick);
            self.pending_count = self.pending.values().map(Vec::len).sum();
        }
        while tick < target {
            tick += 1;
            self.particles.retain_mut(|particle| {
                particle.tick_with(|bounds, movement| {
                    crate::consume_particles::collide(world, &mut self.shapes, bounds, movement)
                })
            });
            let ready = self.pending.split_off(&tick.saturating_add(1));
            let events = std::mem::replace(&mut self.pending, ready);
            for spawn in events.into_values().flatten() {
                self.pending_count -= 1;
                if self.particles.len() == 4096 {
                    self.particles.pop_front();
                }
                let mut random = LegacyRandom::new(self.random.next_long());
                self.particles.push_back(Soul::new(spawn, &mut random));
            }
        }
        self.tick = Some(tick);
    }
    pub fn particles(&self) -> impl Iterator<Item = &Soul> {
        self.particles.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn soul_particle_physics_matches_java() {
        use native_world::movement::{CollisionShape, resolve_movement};
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
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SoulParticleOracle.java");
        let output = temp.path().join("soul.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/SoulParticleOracle.java"),
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
                .arg("SoulParticleOracle")
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
        assert_eq!(rows.len(), 200);
        let ceiling = CollisionShape::new(vec![BoundingBox {
            min: DVec3::new(-100.0, 4.0, -100.0),
            max: DVec3::new(100.0, 5.0, 100.0),
        }]);
        for row in rows {
            let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let spawn = SoulParticleSpawn {
                tick: 1,
                position: DVec3::new(1.25, 3.5, -2.25),
                velocity: DVec3::new(0.04, 0.1, -0.02),
            };
            let mut particle = Soul::new(spawn, &mut random);
            assert_eq!(
                particle.lifetime,
                row["lifetime"].as_i64().unwrap() as i32,
                "{row}"
            );
            assert_eq!(
                particle.size.to_bits(),
                row["size_bits"].as_i64().unwrap() as i32 as u32,
                "{row}"
            );
            for (index, state) in row["states"].as_array().unwrap().iter().enumerate() {
                let alive = if index == 0 {
                    true
                } else {
                    particle.tick_with(|bounds, velocity| {
                        if row["ceiling"].as_bool().unwrap() {
                            resolve_movement(
                                bounds,
                                velocity,
                                false,
                                0.0,
                                std::slice::from_ref(&ceiling),
                            )
                        } else {
                            velocity
                        }
                    })
                };
                for (actual, expected) in particle
                    .position
                    .to_array()
                    .into_iter()
                    .chain(particle.velocity.to_array())
                    .zip(state.as_array().unwrap())
                {
                    assert_eq!(actual.to_bits(), expected.as_i64().unwrap() as u64, "{row}");
                }
                assert_eq!(particle.age, state[6].as_i64().unwrap() as i32, "{row}");
                assert_eq!(!alive, state[7].as_bool().unwrap(), "{row}");
                if alive {
                    assert_eq!(
                        particle.frame(11),
                        state[8].as_u64().unwrap() as usize,
                        "{row}"
                    );
                }
            }
        }
    }
}
