//! Large smoke produced when water evaporates.

use glam::DVec3;
use native_core::random::LegacyRandom;
use native_session::{SessionWorld, loaded::EvaporationSmoke};
use native_world::{collision::Aabb, movement::BoundingBox};
use std::collections::{BTreeMap, HashMap, VecDeque};

#[derive(Clone, Debug)]
pub(crate) struct Smoke {
    pub previous: DVec3,
    pub position: DVec3,
    velocity: DVec3,
    bounds: BoundingBox,
    pub size: f32,
    pub color: f32,
    pub age: i32,
    pub lifetime: i32,
    stopped: bool,
    on_ground: bool,
}

impl Smoke {
    fn new(position: DVec3, random: &mut LegacyRandom) -> Self {
        let _initial_lifetime = (4.0_f32 / (random.next_float() * 0.9 + 0.1)) as i32;
        let mut component = || f64::from((random.next_float() * 2.0 - 1.0) * 0.4);
        let base = DVec3::new(component(), component(), component());
        let speed = f64::from((random.next_float() + random.next_float() + 1.0) * 0.15);
        let length = (base.x * base.x + base.y * base.y + base.z * base.z).sqrt();
        let velocity = DVec3::new(
            base.x / length * speed * f64::from(0.4_f32),
            base.y / length * speed * f64::from(0.4_f32) + f64::from(0.1_f32),
            base.z / length * speed * f64::from(0.4_f32),
        ) * f64::from(0.1_f32);
        let size = (0.1_f32 * (random.next_float() * 0.5 + 0.5) * 2.0) * (0.75_f32 * 2.5);
        let color = random.next_float() * 0.3_f32;
        let lifetime = ((8.0_f64 / (f64::from(random.next_float()) * 0.8 + 0.2)) * 2.5) as i32;
        let half = f64::from(0.1_f32);
        Self {
            previous: position,
            position,
            velocity,
            bounds: BoundingBox {
                min: position + DVec3::new(-half, 0.0, -half),
                max: position + DVec3::new(half, f64::from(0.2_f32), half),
            },
            size,
            color,
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
        self.velocity.y -= 0.04 * f64::from(-0.1_f32);
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
        if self.position.y == self.previous.y {
            self.velocity.x *= 1.1;
            self.velocity.z *= 1.1;
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
    pub fn display_size(&self, partial: f32) -> f32 {
        self.size * (((self.age as f32 + partial) / self.lifetime as f32) * 32.0).clamp(0.0, 1.0)
    }
}

pub(crate) struct SmokeSystem {
    particles: VecDeque<Smoke>,
    pending: BTreeMap<u64, Vec<EvaporationSmoke>>,
    tick: Option<u64>,
    packet_random: LegacyRandom,
    particle_random: LegacyRandom,
    shapes: HashMap<u32, Option<Vec<Aabb>>>,
}

impl Default for SmokeSystem {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |v| v.as_nanos() as i64);
        Self {
            particles: VecDeque::new(),
            pending: BTreeMap::new(),
            tick: None,
            packet_random: LegacyRandom::new(seed),
            particle_random: LegacyRandom::new(seed.wrapping_add(1)),
            shapes: HashMap::new(),
        }
    }
}

impl SmokeSystem {
    pub fn update(&mut self, target: u64, world: &SessionWorld, events: Vec<EvaporationSmoke>) {
        for event in events {
            self.pending.entry(event.tick).or_default().push(event);
        }
        let first = self
            .pending
            .first_key_value()
            .map_or(target, |(&tick, _)| tick.saturating_sub(1).min(target));
        let mut tick = self.tick.unwrap_or(first);
        if target < tick {
            self.particles.clear();
            self.pending.clear();
            self.shapes.clear();
            tick = target;
        }
        if target.saturating_sub(tick) > 200 {
            self.particles.clear();
            tick = target - 200;
            self.pending.retain(|&t, _| t > tick);
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
            for event in events.into_values().flatten() {
                for _ in 0..8 {
                    if self.particles.len() == 4096 {
                        self.particles.pop_front();
                    }
                    let position = event.position.as_dvec3()
                        + DVec3::new(
                            self.packet_random.next_double(),
                            self.packet_random.next_double(),
                            self.packet_random.next_double(),
                        );
                    let mut random = LegacyRandom::new(self.particle_random.next_long());
                    self.particles.push_back(Smoke::new(position, &mut random));
                }
            }
        }
        self.tick = Some(tick);
    }
    pub fn particles(&self) -> impl Iterator<Item = &Smoke> {
        self.particles.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;
    #[test]
    fn smoke_bursts_keep_tick_order_across_frame_batches() {
        let world = SessionWorld::Demo(native_world::World::demo());
        let mut a = SmokeSystem::default();
        let mut b = SmokeSystem::default();
        a.packet_random = LegacyRandom::new(1);
        b.packet_random = LegacyRandom::new(1);
        a.particle_random = LegacyRandom::new(2);
        b.particle_random = LegacyRandom::new(2);
        let burst = |tick| EvaporationSmoke {
            tick,
            position: IVec3::new(2, 4, 3),
        };
        a.update(10, &world, vec![burst(2), burst(6)]);
        for tick in 1..=10 {
            b.update(
                tick,
                &world,
                if tick == 2 || tick == 6 {
                    vec![burst(tick)]
                } else {
                    vec![]
                },
            );
        }
        let fields = |system: &SmokeSystem| {
            system
                .particles()
                .map(|p| (p.position, p.velocity, p.size, p.color, p.age, p.lifetime))
                .collect::<Vec<_>>()
        };
        assert_eq!(fields(&a), fields(&b));
        assert_eq!(a.particles().count(), 16);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn large_smoke_physics_matches_java() {
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
        let source = temp.path().join("SmokeParticleOracle.java");
        let output = temp.path().join("smoke.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/SmokeParticleOracle.java"),
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
                .arg("SmokeParticleOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java oracle failed: {}\n{}",
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
            if let Some(positions) = row
                .get("packet_positions")
                .and_then(serde_json::Value::as_array)
            {
                let mut packet_random = LegacyRandom::new(123);
                for expected in positions {
                    assert_eq!(
                        packet_random.next_double().to_bits(),
                        expected.as_i64().unwrap() as u64
                    );
                }
            }
            let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let mut p = Smoke::new(DVec3::new(1.25, 3.5, -2.25), &mut random);
            assert_eq!(
                p.lifetime,
                row["lifetime"].as_i64().unwrap() as i32,
                "{row}"
            );
            assert_eq!(
                p.size.to_bits(),
                row["size_bits"].as_i64().unwrap() as i32 as u32,
                "{row}"
            );
            assert_eq!(
                p.color.to_bits(),
                row["color_bits"].as_i64().unwrap() as i32 as u32,
                "{row}"
            );
            for (index, state) in row["states"].as_array().unwrap().iter().enumerate() {
                let alive = if index == 0 {
                    true
                } else {
                    p.tick_with(|bounds, velocity| {
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
                for (actual, expected) in p
                    .position
                    .to_array()
                    .into_iter()
                    .chain(p.velocity.to_array())
                    .zip(state.as_array().unwrap())
                {
                    assert_eq!(actual.to_bits(), expected.as_i64().unwrap() as u64, "{row}");
                }
                assert_eq!(p.age, state[6].as_i64().unwrap() as i32, "{row}");
                assert_eq!(!alive, state[7].as_bool().unwrap(), "{row}");
                assert_eq!(
                    p.display_size(0.5).to_bits(),
                    state[8].as_i64().unwrap() as i32 as u32,
                    "{row}"
                );
                if alive {
                    assert_eq!(p.frame(8), state[9].as_u64().unwrap() as usize, "{row}");
                }
            }
        }
    }
}
