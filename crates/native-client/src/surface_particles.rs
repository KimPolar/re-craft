//! Client-side particles sent by bubble columns at the water surface.

use std::collections::{BTreeMap, HashMap, VecDeque};

use glam::DVec3;
use native_core::random::LegacyRandom;
use native_session::{
    SessionWorld,
    loaded::{BubbleSurfaceParticle, BubbleSurfaceParticleKind, DrownParticleSpawn},
};
use native_world::{collision::Aabb, fluid::FluidState, movement::BoundingBox};

const LIMIT: usize = 4096;

/// The packet listener has its own Gaussian random stream, separate from the world.
pub(crate) struct SurfacePacketRandom {
    random: LegacyRandom,
    cached: Option<f64>,
}

impl Default for SurfacePacketRandom {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos() as i64);
        Self::new(seed)
    }
}

impl SurfacePacketRandom {
    fn new(seed: i64) -> Self {
        Self {
            random: LegacyRandom::new(seed),
            cached: None,
        }
    }

    fn gaussian(&mut self) -> f64 {
        if let Some(value) = self.cached.take() {
            return value;
        }
        loop {
            let x = 2.0 * self.random.next_double() - 1.0;
            let y = 2.0 * self.random.next_double() - 1.0;
            let radius = x * x + y * y;
            if radius < 1.0 && radius != 0.0 {
                let scale = (-2.0 * radius.ln() / radius).sqrt();
                self.cached = Some(y * scale);
                return x * scale;
            }
        }
    }

    pub fn decode(&mut self, packet: BubbleSurfaceParticle) -> DrownParticleSpawn {
        let (y_dist, speed) = match packet.kind {
            BubbleSurfaceParticleKind::Splash => (0.0_f32, 1.0_f32),
            BubbleSurfaceParticleKind::Bubble => (0.01_f32, 0.2_f32),
        };
        let x = self.gaussian() * 0.0;
        let y = self.gaussian() * f64::from(y_dist);
        let z = self.gaussian() * 0.0;
        let velocity =
            DVec3::new(self.gaussian(), self.gaussian(), self.gaussian()) * f64::from(speed);
        DrownParticleSpawn {
            tick: packet.tick,
            position: packet.position + DVec3::new(x, y, z),
            velocity,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Splash {
    pub previous: DVec3,
    pub position: DVec3,
    velocity: DVec3,
    bounds: BoundingBox,
    pub size: f32,
    pub sprite: usize,
    lifetime: i32,
    stopped: bool,
    on_ground: bool,
    random: LegacyRandom,
}

impl Splash {
    fn new(spawn: DrownParticleSpawn, sprite: usize, mut random: LegacyRandom) -> Self {
        // The velocity-taking base Particle constructor starts with zero input.
        let _base_lifetime = (4.0_f32 / (random.next_float() * 0.9_f32 + 0.1_f32)) as i32;
        let mut component = || f64::from((random.next_float() * 2.0_f32 - 1.0_f32) * 0.4_f32);
        let base = DVec3::new(component(), component(), component());
        let magnitude = f64::from((random.next_float() + random.next_float() + 1.0_f32) * 0.15_f32);
        let length = (base.x * base.x + base.y * base.y + base.z * base.z).sqrt();
        let size = 0.1_f32 * (random.next_float() * 0.5_f32 + 0.5_f32) * 2.0_f32;
        let mut velocity = DVec3::new(
            base.x / length * magnitude * f64::from(0.4_f32) * f64::from(0.3_f32),
            f64::from(random.next_float() * 0.2_f32 + 0.1_f32),
            base.z / length * magnitude * f64::from(0.4_f32) * f64::from(0.3_f32),
        );
        let lifetime = (8.0 / (f64::from(random.next_float()) * 0.8 + 0.2)) as i32;
        if spawn.velocity.y == 0.0 && (spawn.velocity.x != 0.0 || spawn.velocity.z != 0.0) {
            velocity = DVec3::new(spawn.velocity.x, 0.1, spawn.velocity.z);
        }
        let old_half = f64::from(0.1_f32);
        let width = f64::from(0.01_f32);
        let min_x = ((spawn.position.x - old_half) + (spawn.position.x + old_half) - width) / 2.0;
        let min_z = ((spawn.position.z - old_half) + (spawn.position.z + old_half) - width) / 2.0;
        Self {
            previous: spawn.position,
            position: spawn.position,
            velocity,
            bounds: BoundingBox {
                min: DVec3::new(min_x, spawn.position.y, min_z),
                max: DVec3::new(min_x + width, spawn.position.y + width, min_z + width),
            },
            size,
            sprite,
            lifetime,
            stopped: false,
            on_ground: false,
            random,
        }
    }

    fn tick_with(
        &mut self,
        mut resolve: impl FnMut(BoundingBox, DVec3) -> DVec3,
        mut submerged: impl FnMut(DVec3) -> bool,
    ) -> bool {
        self.previous = self.position;
        let lifetime = self.lifetime;
        self.lifetime -= 1;
        if lifetime <= 0 {
            return false;
        }
        self.velocity.y -= f64::from(0.04_f32);
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
        self.velocity *= f64::from(0.98_f32);
        let mut alive = true;
        if self.on_ground {
            alive = self.random.next_float() >= 0.5_f32;
            self.velocity.x *= f64::from(0.7_f32);
            self.velocity.z *= f64::from(0.7_f32);
        }
        alive && !submerged(self.position)
    }
}

fn submerged(
    world: &SessionWorld,
    position: DVec3,
    shapes: &mut HashMap<u32, Option<Vec<Aabb>>>,
) -> bool {
    let cell = position.floor().as_ivec3();
    let local = position - cell.as_dvec3();
    let (shape, fluid_height) = match world {
        SessionWorld::Demo(world) => (native_world::collision::demo_shape(world.block(cell)), 0.0),
        SessionWorld::Loaded(world) => {
            let Some(id) = world.block_id(cell.x, cell.y, cell.z) else {
                return false;
            };
            let Some(state) = world.state(id) else {
                return false;
            };
            let fluid = FluidState::from_block(state).ok().flatten();
            let fluid_height = fluid.map_or(0.0, |fluid| {
                let above = cell
                    .y
                    .checked_add(1)
                    .and_then(|y| world.block_id(cell.x, y, cell.z))
                    .and_then(|id| world.state(id))
                    .and_then(|state| FluidState::from_block(state).ok().flatten());
                if above.is_some_and(|above| above.kind == fluid.kind) {
                    1.0
                } else {
                    f64::from(fluid.own_height())
                }
            });
            let shape = shapes
                .entry(id.0)
                .or_insert_with(|| native_world::collision::block_shape(state));
            (shape.as_deref().unwrap_or(&[]), fluid_height)
        }
    };
    let shape_height = shape
        .iter()
        .filter(|a| {
            local.x >= f64::from(a.min.x)
                && local.x < f64::from(a.max.x)
                && local.z >= f64::from(a.min.z)
                && local.z < f64::from(a.max.z)
        })
        .map(|a| f64::from(a.max.y))
        .fold(0.0_f64, f64::max);
    let height = shape_height.max(fluid_height);
    height > 0.0 && position.y < f64::from(cell.y) + height
}

pub(crate) struct SplashSystem {
    particles: VecDeque<Splash>,
    pending: BTreeMap<u64, Vec<DrownParticleSpawn>>,
    pending_count: usize,
    tick: Option<u64>,
    sprite_random: LegacyRandom,
    particle_random: LegacyRandom,
    shapes: HashMap<u32, Option<Vec<Aabb>>>,
    surface_shapes: HashMap<u32, Option<Vec<Aabb>>>,
}

impl Default for SplashSystem {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos() as i64);
        Self {
            particles: VecDeque::new(),
            pending: BTreeMap::new(),
            pending_count: 0,
            tick: None,
            sprite_random: LegacyRandom::new(seed),
            particle_random: LegacyRandom::new(seed.wrapping_add(1)),
            shapes: HashMap::new(),
            surface_shapes: HashMap::new(),
        }
    }
}

impl SplashSystem {
    pub fn enqueue(&mut self, event: DrownParticleSpawn) {
        if self.pending_count < LIMIT {
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
            self.surface_shapes.clear();
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
                    |position| submerged(world, position, &mut self.surface_shapes),
                )
            });
            let future = self.pending.split_off(&tick.saturating_add(1));
            let ready = std::mem::replace(&mut self.pending, future);
            for spawn in ready.into_values().flatten() {
                self.pending_count -= 1;
                if sprite_count == 0 {
                    continue;
                }
                if self.particles.len() == LIMIT {
                    self.particles.pop_front();
                }
                let sprite = self
                    .sprite_random
                    .next_int_bound(sprite_count as i32)
                    .expect("nonempty sprites") as usize;
                let random = LegacyRandom::new(self.particle_random.next_long());
                self.particles.push_back(Splash::new(spawn, sprite, random));
            }
        }
        self.tick = Some(tick);
    }

    pub fn particles(&self) -> impl Iterator<Item = &Splash> {
        self.particles.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splash_removal_uses_fluid_height_and_local_collision_shape() {
        use native_world::loaded::{BLOCK_COUNT, BlockState, Chunk, ChunkPos, Section, World};
        use std::sync::Arc;
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: Default::default(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: Default::default(),
        });
        let slab = world.intern_state(BlockState {
            name: "minecraft:stone_slab".into(),
            properties: [
                ("type".into(), "bottom".into()),
                ("waterlogged".into(), "false".into()),
            ]
            .into(),
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        let index = |x: usize, y: usize, z: usize| x + z * 16 + y * 256;
        blocks[index(1, 1, 1)] = water;
        blocks[index(2, 1, 1)] = water;
        blocks[index(2, 2, 1)] = water;
        blocks[index(3, 1, 1)] = slab;
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:full".into()),
                block_light_correct: false,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let world = SessionWorld::Loaded(Arc::new(world));
        let mut cache = HashMap::new();
        assert!(submerged(&world, DVec3::new(1.5, 1.88, 1.5), &mut cache));
        assert!(!submerged(&world, DVec3::new(1.5, 1.90, 1.5), &mut cache));
        assert!(submerged(&world, DVec3::new(2.5, 1.99, 1.5), &mut cache));
        assert!(submerged(&world, DVec3::new(3.5, 1.49, 1.5), &mut cache));
        assert!(!submerged(&world, DVec3::new(3.5, 1.50, 1.5), &mut cache));
    }

    #[test]
    fn pending_splashes_are_bounded_and_frame_batch_independent() {
        let spawn = DrownParticleSpawn {
            tick: 1,
            position: DVec3::new(0.5, 10.0, 0.5),
            velocity: DVec3::new(0.2, 0.8, -0.4),
        };
        let world = SessionWorld::Demo(native_world::World::default());
        let mut single = SplashSystem::default();
        single.sprite_random = LegacyRandom::new(7);
        single.particle_random = LegacyRandom::new(8);
        let mut batched = SplashSystem::default();
        batched.sprite_random = LegacyRandom::new(7);
        batched.particle_random = LegacyRandom::new(8);
        for i in 0..5 {
            let event = DrownParticleSpawn {
                tick: i + 1,
                ..spawn
            };
            single.enqueue(event);
            batched.enqueue(event);
        }
        for tick in 1..=5 {
            single.update(tick, &world, 4);
        }
        batched.update(5, &world, 4);
        let state = |system: &SplashSystem| {
            system
                .particles()
                .map(|p| (p.position, p.velocity, p.lifetime, p.sprite))
                .collect::<Vec<_>>()
        };
        assert_eq!(state(&single), state(&batched));
        for _ in 0..(LIMIT + 1) {
            single.enqueue(spawn);
        }
        assert_eq!(single.pending_count, LIMIT);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn splash_physics_and_packet_randomness_match_java() {
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SplashParticleOracle.java");
        let output = temp.path().join("splash.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/SplashParticleOracle.java"),
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
                .arg("SplashParticleOracle")
                .arg(&output),
        ] {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let output: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let floor = CollisionShape::new(vec![BoundingBox {
            min: DVec3::new(-100.0, 2.0, -100.0),
            max: DVec3::new(100.0, 3.0, 100.0),
        }]);
        let rows = output["particles"].as_array().unwrap();
        assert_eq!(rows.len(), 400);
        for row in rows {
            let mode = row["mode"].as_u64().unwrap();
            let spawn = DrownParticleSpawn {
                tick: 1,
                position: DVec3::new(1.25, 3.5, -2.25),
                velocity: DVec3::new(0.2, if mode == 3 { 0.0 } else { 0.8 }, -0.4),
            };
            let mut particle =
                Splash::new(spawn, 0, LegacyRandom::new(row["seed"].as_i64().unwrap()));
            assert_eq!(
                particle.size.to_bits(),
                row["size_bits"].as_i64().unwrap() as u32,
                "{row}"
            );
            for (index, expected) in row["states"].as_array().unwrap().iter().enumerate() {
                let alive = if index == 0 {
                    true
                } else {
                    particle.tick_with(
                        |bounds, movement| {
                            if mode == 1 {
                                resolve_movement(
                                    bounds,
                                    movement,
                                    false,
                                    0.0,
                                    std::slice::from_ref(&floor),
                                )
                            } else {
                                movement
                            }
                        },
                        |pos| {
                            mode == 2
                                && pos.y.floor() == 2.0
                                && pos.y < 2.0 + f64::from(8.0_f32 / 9.0_f32)
                        },
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
                assert_eq!(
                    alive,
                    values[7].as_bool().unwrap(),
                    "mode={mode} tick={index} row={row}"
                );
            }
        }
        let mut random = SurfacePacketRandom::new(77);
        for (index, row) in output["packets"].as_array().unwrap().iter().enumerate() {
            let packet = BubbleSurfaceParticle {
                tick: 1,
                position: DVec3::new(1.25, 3.5, -2.25),
                kind: if index % 2 == 0 {
                    BubbleSurfaceParticleKind::Splash
                } else {
                    BubbleSurfaceParticleKind::Bubble
                },
            };
            let spawn = random.decode(packet);
            let actual = [
                spawn.position.x,
                spawn.position.y,
                spawn.position.z,
                spawn.velocity.x,
                spawn.velocity.y,
                spawn.velocity.z,
            ];
            for (actual, expected) in actual.iter().zip(row.as_array().unwrap()) {
                assert!(
                    (actual - expected.as_f64().unwrap()).abs() < 1.0e-14,
                    "packet={index} actual={actual} expected={expected}"
                );
            }
        }
        assert_eq!(
            random.random.next_int(),
            output["next"].as_i64().unwrap() as i32
        );
    }
}
