//! Client-side 26.3 drowning bubbles. Entity-event randomness is handled by
//! the session; each rendered particle has its own independent random source.

use glam::DVec3;
use native_core::random::LegacyRandom;
use native_session::{SessionWorld, loaded::DrownParticleSpawn};
use native_world::{
    collision::Aabb,
    fluid::{FluidKind, FluidState},
    movement::BoundingBox,
};
use std::collections::{BTreeMap, HashMap, VecDeque};

const MAX_PARTICLES: usize = 4096;

#[derive(Clone, Debug)]
pub(crate) struct Bubble {
    pub previous: DVec3,
    pub position: DVec3,
    velocity: DVec3,
    bounds: BoundingBox,
    pub size: f32,
    pub sprite: usize,
    lifetime: i32,
    stopped: bool,
}

impl Bubble {
    fn new(spawn: DrownParticleSpawn, sprite: usize, random: &mut LegacyRandom) -> Self {
        let _base_lifetime = (4.0_f32 / (random.next_float() * 0.9_f32 + 0.1_f32)) as i32;
        let size = (0.1_f32 * (random.next_float() * 0.5_f32 + 0.5_f32) * 2.0_f32)
            * (random.next_float() * 0.6_f32 + 0.2_f32);
        let mut component = |v: f64| {
            v * f64::from(0.2_f32) + f64::from((random.next_float() * 2.0_f32 - 1.0_f32) * 0.02_f32)
        };
        let velocity = DVec3::new(
            component(spawn.velocity.x),
            component(spawn.velocity.y),
            component(spawn.velocity.z),
        );
        let lifetime = (8.0_f64 / (f64::from(random.next_float()) * 0.8_f64 + 0.2_f64)) as i32;
        // Particle.setSize resizes the existing 0.2-wide box around its center.
        let old_half = f64::from(0.1_f32);
        let width = f64::from(0.02_f32);
        let min_x = ((spawn.position.x - old_half) + (spawn.position.x + old_half) - width) / 2.0;
        let min_z = ((spawn.position.z - old_half) + (spawn.position.z + old_half) - width) / 2.0;
        Self {
            previous: spawn.position,
            position: spawn.position,
            velocity,
            size,
            sprite,
            lifetime,
            bounds: BoundingBox {
                min: DVec3::new(min_x, spawn.position.y, min_z),
                max: DVec3::new(min_x + width, spawn.position.y + width, min_z + width),
            },
            stopped: false,
        }
    }

    fn tick_with(
        &mut self,
        mut resolve: impl FnMut(BoundingBox, DVec3) -> DVec3,
        mut is_water: impl FnMut(DVec3) -> bool,
    ) -> bool {
        self.previous = self.position;
        let lifetime = self.lifetime;
        self.lifetime -= 1;
        if lifetime <= 0 {
            return false;
        }
        self.velocity.y += 0.002_f64;
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
            if requested.x != actual.x {
                self.velocity.x = 0.0;
            }
            if requested.z != actual.z {
                self.velocity.z = 0.0;
            }
        }
        self.velocity *= f64::from(0.85_f32);
        is_water(self.position)
    }
}

fn water_at(world: &SessionWorld, position: DVec3) -> bool {
    let cell = position.floor().as_ivec3();
    match world {
        SessionWorld::Demo(_) => false,
        SessionWorld::Loaded(world) => world
            .block_id(cell.x, cell.y, cell.z)
            .and_then(|id| world.state(id))
            .and_then(|state| FluidState::from_block(state).ok().flatten())
            .is_some_and(|fluid| fluid.kind == FluidKind::Water),
    }
}

pub(crate) struct BubbleSystem {
    particles: VecDeque<Bubble>,
    pending: BTreeMap<u64, Vec<DrownParticleSpawn>>,
    pending_count: usize,
    tick: Option<u64>,
    sprite_random: LegacyRandom,
    particle_random: LegacyRandom,
    shapes: HashMap<u32, Option<Vec<Aabb>>>,
}

impl Default for BubbleSystem {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |value| value.as_nanos() as i64);
        Self {
            particles: VecDeque::new(),
            pending: BTreeMap::new(),
            pending_count: 0,
            tick: None,
            sprite_random: LegacyRandom::new(seed),
            particle_random: LegacyRandom::new(seed.wrapping_add(1)),
            shapes: HashMap::new(),
        }
    }
}

impl BubbleSystem {
    pub fn enqueue(&mut self, events: Vec<DrownParticleSpawn>) {
        for event in events {
            if self.pending_count == MAX_PARTICLES {
                break;
            }
            self.pending.entry(event.tick).or_default().push(event);
            self.pending_count += 1;
        }
    }

    pub fn update(&mut self, target: u64, world: &SessionWorld, sprite_count: usize) {
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
        if target.saturating_sub(tick) > 100 {
            self.particles.clear();
            tick = target - 100;
            self.pending.retain(|&t, _| t > tick);
            self.pending_count = self.pending.values().map(Vec::len).sum();
        }
        while tick < target {
            tick += 1;
            self.particles.retain_mut(|particle| {
                particle.tick_with(
                    |bounds, movement| {
                        crate::consume_particles::collide(world, &mut self.shapes, bounds, movement)
                    },
                    |position| water_at(world, position),
                )
            });
            let ready = self.pending.split_off(&tick.saturating_add(1));
            let events = std::mem::replace(&mut self.pending, ready);
            for spawn in events.into_values().flatten() {
                self.pending_count -= 1;
                if sprite_count == 0 {
                    continue;
                }
                if self.particles.len() == MAX_PARTICLES {
                    self.particles.pop_front();
                }
                let sprite = self
                    .sprite_random
                    .next_int_bound(sprite_count as i32)
                    .expect("nonempty sprite set") as usize;
                let mut random = LegacyRandom::new(self.particle_random.next_long());
                self.particles
                    .push_back(Bubble::new(spawn, sprite, &mut random));
            }
        }
        self.tick = Some(tick);
    }

    pub fn particles(&self) -> impl Iterator<Item = &Bubble> {
        self.particles.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bubbles_are_removed_outside_water_and_pending_events_are_bounded() {
        let event = DrownParticleSpawn {
            tick: 1,
            position: DVec3::new(1.0, 2.0, 3.0),
            velocity: DVec3::ZERO,
        };
        let mut bubble = Bubble::new(event, 0, &mut LegacyRandom::new(1));
        assert!(bubble.tick_with(|_, movement| movement, |_| true));
        assert!(!bubble.tick_with(|_, movement| movement, |_| false));
        let mut system = BubbleSystem::default();
        system.enqueue(vec![event; MAX_PARTICLES + 100]);
        assert_eq!(system.pending_count, MAX_PARTICLES);
        let world = SessionWorld::Demo(native_world::World::default());
        system.update(1, &world, 1);
        assert_eq!(system.particles().count(), MAX_PARTICLES);
        system.update(2, &world, 1);
        assert_eq!(system.particles().count(), 0);
        assert_eq!(system.pending_count, 0);
    }

    #[test]
    fn bubbles_match_across_frame_batches_in_water() {
        use native_world::loaded::{BLOCK_COUNT, BlockState, Chunk, ChunkPos, Section, World};
        use std::sync::Arc;
        let mut water = World::new("minecraft:overworld");
        let state = water.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: Default::default(),
        });
        let pos = ChunkPos { x: 0, z: 0 };
        water.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:full".into()),
                block_light_correct: false,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(Box::new([state; BLOCK_COUNT])),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let world = SessionWorld::Loaded(Arc::new(water));
        let event = |tick| DrownParticleSpawn {
            tick,
            position: DVec3::new(3.5, 3.5, 3.5),
            velocity: DVec3::new(0.1, -0.1, 0.2),
        };
        let mut batched = BubbleSystem::default();
        let mut individual = BubbleSystem::default();
        batched.sprite_random = LegacyRandom::new(2);
        individual.sprite_random = LegacyRandom::new(2);
        batched.particle_random = LegacyRandom::new(3);
        individual.particle_random = LegacyRandom::new(3);
        batched.enqueue(vec![event(2), event(6)]);
        batched.update(10, &world, 3);
        for tick in 1..=10 {
            if tick == 2 || tick == 6 {
                individual.enqueue(vec![event(tick)]);
            }
            individual.update(tick, &world, 3);
        }
        let fields = |s: &BubbleSystem| {
            s.particles()
                .map(|p| (p.position, p.velocity, p.size, p.sprite, p.lifetime))
                .collect::<Vec<_>>()
        };
        assert_eq!(batched.particles().count(), 2);
        assert_eq!(fields(&batched), fields(&individual));
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn bubble_creation_motion_and_expiration_match_java() {
        use native_world::movement::{CollisionShape, resolve_movement};
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("BubbleParticleOracle.java");
        let output = temp.path().join("bubble.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/BubbleParticleOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("BubbleParticleOracle")
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 300);
        let ceiling = CollisionShape::new(vec![BoundingBox {
            min: DVec3::new(-100.0, 4.0, -100.0),
            max: DVec3::new(100.0, 5.0, 100.0),
        }]);
        for row in rows {
            let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let spawn = DrownParticleSpawn {
                tick: 1,
                position: DVec3::new(1.25, 3.5, -2.25),
                velocity: DVec3::new(0.2, 0.8, -0.4),
            };
            let mut particle = Bubble::new(spawn, 0, &mut random);
            assert_eq!(
                particle.size.to_bits(),
                row["size_bits"].as_i64().unwrap() as u32,
                "{row}"
            );
            let mode = row["mode"].as_u64().unwrap();
            for (index, expected) in row["states"].as_array().unwrap().iter().enumerate() {
                let alive = if index == 0 {
                    true
                } else {
                    particle.tick_with(
                        |bounds, motion| {
                            if mode == 1 {
                                resolve_movement(
                                    bounds,
                                    motion,
                                    false,
                                    0.0,
                                    std::slice::from_ref(&ceiling),
                                )
                            } else {
                                motion
                            }
                        },
                        |_| mode != 2 || index <= 3,
                    )
                };
                let values = expected.as_array().unwrap();
                let actual = [
                    particle.position.x,
                    particle.position.y,
                    particle.position.z,
                    particle.velocity.x,
                    particle.velocity.y,
                    particle.velocity.z,
                ];
                for (actual, expected) in actual.iter().zip(values) {
                    assert_eq!(
                        actual.to_bits(),
                        expected.as_i64().unwrap() as u64,
                        "mode={mode} tick={index} row={row}"
                    );
                }
                assert_eq!(
                    particle.lifetime,
                    values[6].as_i64().unwrap() as i32,
                    "{row}"
                );
                assert_eq!(alive, values[7].as_bool().unwrap(), "{row}");
            }
        }
    }
}
