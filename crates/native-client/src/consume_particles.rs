//! Client-side food fragments, advanced on the same fixed ticks as the world.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::Arc,
};

use glam::{DVec3, IVec3};
use native_core::random::LegacyRandom;
use native_session::{
    SessionWorld,
    consume_particles::ConsumeParticleSpawn,
    inventory::ItemStack,
    loaded::{PlantBreakParticles, PlantParticleKind},
};
use native_world::{
    collision::Aabb,
    movement::{BoundingBox, CollisionShape, resolve_movement},
};

const MAX_PARTICLES: usize = 4096;

/// 26.3 subdivides the plants' 12×13×12 voxel shape at quarter-block intervals.
fn plant_fragments(block: IVec3) -> impl Iterator<Item = (DVec3, DVec3)> {
    (0..3).flat_map(move |x| {
        (0..4).flat_map(move |y| {
            (0..3).map(move |z| {
                let fraction = DVec3::new(
                    (f64::from(x) + 0.5) / 3.0,
                    (f64::from(y) + 0.5) / 4.0,
                    (f64::from(z) + 0.5) / 3.0,
                );
                let local = DVec3::new(
                    0.125 + fraction.x * 0.75,
                    fraction.y * 0.8125,
                    0.125 + fraction.z * 0.75,
                );
                (block.as_dvec3() + local, fraction - DVec3::splat(0.5))
            })
        })
    })
}

#[derive(Clone, Debug)]
pub(crate) struct Particle {
    pub visual: ParticleVisual,
    pub previous: DVec3,
    pub position: DVec3,
    pub velocity: DVec3,
    pub size: f32,
    pub uv: [f32; 2],
    bounds: BoundingBox,
    age: i32,
    lifetime: i32,
    stopped: bool,
    on_ground: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum ParticleVisual {
    Item(Arc<ItemStack>),
    Plant(PlantParticleKind, IVec3),
}

impl Particle {
    fn new(spawn: ConsumeParticleSpawn, random: &mut LegacyRandom) -> Self {
        Self::new_inner(
            spawn.position,
            spawn.velocity,
            ParticleVisual::Item(spawn.item),
            false,
            random,
        )
    }

    fn new_inner(
        position: DVec3,
        velocity: DVec3,
        visual: ParticleVisual,
        terrain: bool,
        random: &mut LegacyRandom,
    ) -> Self {
        // Particle has a separate random source from the player's random stream.
        let lifetime = (4.0_f32 / (random.next_float() * 0.9 + 0.1)) as i32;
        let mut random_component = || f64::from((random.next_float() * 2.0 - 1.0) * 0.4);
        let initial = DVec3::new(random_component(), random_component(), random_component())
            + if terrain { velocity } else { DVec3::ZERO };
        let speed = f64::from((random.next_float() + random.next_float() + 1.0) * 0.15);
        let length = (initial.x * initial.x + initial.y * initial.y + initial.z * initial.z).sqrt();
        let initial_velocity = DVec3::new(
            initial.x / length * speed * f64::from(0.4_f32),
            initial.y / length * speed * f64::from(0.4_f32) + f64::from(0.1_f32),
            initial.z / length * speed * f64::from(0.4_f32),
        );
        let size = (0.1_f32 * (random.next_float() * 0.5 + 0.5) * 2.0) / 2.0;
        let uv = [random.next_float() * 3.0, random.next_float() * 3.0];
        let half = f64::from(0.1_f32);
        let bounds = BoundingBox {
            min: position + DVec3::new(-half, 0.0, -half),
            max: position + DVec3::new(half, f64::from(0.2_f32), half),
        };
        Self {
            visual,
            previous: position,
            position,
            velocity: if terrain {
                initial_velocity
            } else {
                initial_velocity * f64::from(0.1_f32) + velocity
            },
            size,
            uv,
            bounds,
            age: 0,
            lifetime,
            stopped: false,
            on_ground: false,
        }
    }

    fn tick(&mut self, world: &SessionWorld, cache: &mut HashMap<u32, Option<Vec<Aabb>>>) -> bool {
        self.tick_with(|bounds, movement| collide(world, cache, bounds, movement))
    }

    fn tick_with(&mut self, mut resolve: impl FnMut(BoundingBox, DVec3) -> DVec3) -> bool {
        self.previous = self.position;
        let age = self.age;
        self.age += 1;
        if age >= self.lifetime {
            return false;
        }
        self.velocity.y -= 0.04;
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
        if self.on_ground {
            self.velocity.x *= f64::from(0.7_f32);
            self.velocity.z *= f64::from(0.7_f32);
        }
        true
    }
}

pub(crate) fn collide(
    world: &SessionWorld,
    cache: &mut HashMap<u32, Option<Vec<Aabb>>>,
    bounds: BoundingBox,
    movement: DVec3,
) -> DVec3 {
    if movement == DVec3::ZERO || movement.length_squared() >= 10000.0 {
        return movement;
    }
    let swept = bounds.expanded_towards(movement);
    let start = (swept.min - DVec3::splat(1.0e-7)).floor().as_ivec3();
    let end = (swept.max + DVec3::splat(1.0e-7)).floor().as_ivec3();
    let mut shapes = Vec::new();
    for x in start.x..=end.x {
        for y in start.y.saturating_sub(1)..=end.y {
            for z in start.z..=end.z {
                let cell = IVec3::new(x, y, z);
                let shape: &[Aabb] = match world {
                    SessionWorld::Demo(world) => {
                        native_world::collision::demo_shape(world.block(cell))
                    }
                    SessionWorld::Loaded(world) => {
                        let Some(id) = world.block_id(x, y, z) else {
                            continue;
                        };
                        cache
                            .entry(id.0)
                            .or_insert_with(|| {
                                world
                                    .state(id)
                                    .and_then(native_world::collision::block_shape)
                            })
                            .as_deref()
                            .unwrap_or(&[])
                    }
                };
                if shape.is_empty() {
                    continue;
                }
                let base = cell.as_dvec3();
                shapes.push(CollisionShape::new(
                    shape
                        .iter()
                        .map(|entry| BoundingBox {
                            min: base + entry.min.as_dvec3(),
                            max: base + entry.max.as_dvec3(),
                        })
                        .collect(),
                ));
            }
        }
    }
    resolve_movement(bounds, movement, false, 0.0, &shapes)
}

pub(crate) struct ParticleSystem {
    particles: VecDeque<Particle>,
    pending: BTreeMap<u64, Vec<ConsumeParticleSpawn>>,
    pending_plants: BTreeMap<u64, Vec<PlantBreakParticles>>,
    tick: Option<u64>,
    random: LegacyRandom,
    shapes: HashMap<u32, Option<Vec<Aabb>>>,
}

impl Default for ParticleSystem {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |value| value.as_nanos() as i64);
        Self {
            particles: VecDeque::new(),
            pending: BTreeMap::new(),
            pending_plants: BTreeMap::new(),
            tick: None,
            random: LegacyRandom::new(seed),
            shapes: HashMap::new(),
        }
    }
}

impl ParticleSystem {
    pub fn queue_plant_breaks(&mut self, events: Vec<PlantBreakParticles>) {
        for event in events {
            self.pending_plants
                .entry(event.tick)
                .or_default()
                .push(event);
        }
    }
    pub fn update(&mut self, target: u64, world: &SessionWorld, spawns: Vec<ConsumeParticleSpawn>) {
        for spawn in spawns {
            self.pending.entry(spawn.tick).or_default().push(spawn);
        }
        let first = self
            .pending
            .keys()
            .chain(self.pending_plants.keys())
            .copied()
            .min()
            .map_or(target, |tick| tick.saturating_sub(1).min(target));
        let mut tick = self.tick.unwrap_or(first);
        if target < tick {
            self.particles.clear();
            self.pending.clear();
            self.pending_plants.clear();
            self.shapes.clear();
            tick = target;
        }
        // Particles live at most 40 ticks; stale effects can be discarded after a long gap.
        if target.saturating_sub(tick) > 100 {
            self.particles.clear();
            tick = target - 100;
            self.pending.retain(|&spawn_tick, _| spawn_tick > tick);
            self.pending_plants
                .retain(|&spawn_tick, _| spawn_tick > tick);
        }
        while tick < target {
            tick += 1;
            self.particles
                .retain_mut(|particle| particle.tick(world, &mut self.shapes));
            let ready = self.pending.split_off(&tick.saturating_add(1));
            let events = std::mem::replace(&mut self.pending, ready);
            for spawn in events.into_values().flatten() {
                if self.particles.len() == MAX_PARTICLES {
                    self.particles.pop_front();
                }
                let mut random = LegacyRandom::new(self.random.next_long());
                self.particles.push_back(Particle::new(spawn, &mut random));
            }
            let ready = self.pending_plants.split_off(&tick.saturating_add(1));
            let events = std::mem::replace(&mut self.pending_plants, ready);
            for event in events.into_values().flatten() {
                for (position, velocity) in plant_fragments(event.position) {
                    if self.particles.len() == MAX_PARTICLES {
                        self.particles.pop_front();
                    }
                    let mut random = LegacyRandom::new(self.random.next_long());
                    self.particles.push_back(Particle::new_inner(
                        position,
                        velocity,
                        ParticleVisual::Plant(event.kind, event.position),
                        true,
                        &mut random,
                    ));
                }
            }
        }
        self.tick = Some(tick);
    }

    pub fn particles(&self) -> impl Iterator<Item = &Particle> {
        self.particles.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::Tag;

    fn spawn(tick: u64) -> ConsumeParticleSpawn {
        ConsumeParticleSpawn {
            item: Arc::new(
                ItemStack::new("minecraft:apple".into(), 1, 64, Tag::Compound(vec![])).unwrap(),
            ),
            tick,
            position: DVec3::new(1.25, 3.5, -2.25),
            velocity: DVec3::new(0.01, 0.15, -0.05),
        }
    }

    #[test]
    fn batches_produce_the_same_particles_as_individual_ticks() {
        let world = SessionWorld::Demo(native_world::World::demo());
        let mut a = ParticleSystem::default();
        let mut b = ParticleSystem::default();
        a.random = LegacyRandom::new(42);
        b.random = LegacyRandom::new(42);
        a.update(10, &world, vec![spawn(2), spawn(6)]);
        for tick in 1..=10 {
            b.update(
                tick,
                &world,
                if tick == 2 || tick == 6 {
                    vec![spawn(tick)]
                } else {
                    vec![]
                },
            );
        }
        let fields = |system: &ParticleSystem| {
            system
                .particles()
                .map(|p| (p.position, p.velocity, p.age, p.lifetime, p.size, p.uv))
                .collect::<Vec<_>>()
        };
        assert_eq!(fields(&a), fields(&b));
    }

    #[test]
    fn long_gaps_discard_expired_pending_particles() {
        let world = SessionWorld::Demo(native_world::World::demo());
        let mut particles = ParticleSystem::default();
        particles.update(0, &world, vec![]);
        particles.update(1000, &world, vec![spawn(1), spawn(1000)]);
        assert_eq!(particles.particles().count(), 1);
    }

    #[test]
    fn plant_breaks_keep_particles_across_frame_batches() {
        let world = SessionWorld::Demo(native_world::World::demo());
        let mut batched = ParticleSystem::default();
        let mut individual = ParticleSystem::default();
        batched.random = LegacyRandom::new(42);
        individual.random = LegacyRandom::new(42);
        let burst = |tick| PlantBreakParticles {
            tick,
            position: IVec3::new(2, 4, 3),
            kind: PlantParticleKind::Fern,
        };
        batched.queue_plant_breaks(vec![burst(2), burst(6)]);
        batched.update(10, &world, vec![]);
        for tick in 1..=10 {
            if tick == 2 || tick == 6 {
                individual.queue_plant_breaks(vec![burst(tick)]);
            }
            individual.update(tick, &world, vec![]);
        }
        let fields = |system: &ParticleSystem| {
            system
                .particles()
                .map(|p| (p.position, p.velocity, p.age, p.lifetime, p.size, p.uv))
                .collect::<Vec<_>>()
        };
        assert_eq!(fields(&batched), fields(&individual));
        assert!(batched.particles().count() > 0);
        let mut initial = ParticleSystem::default();
        initial.queue_plant_breaks(vec![burst(2)]);
        initial.update(2, &world, vec![]);
        assert_eq!(initial.particles().count(), 36);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn plant_shapes_and_particle_physics_match_java() {
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
                let file = game.join("libraries").join(path);
                if file.is_file() {
                    paths.push(file);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("PlantParticleOracle.java");
        let output = temp.path().join("plant_particles.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/PlantParticleOracle.java"),
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
                .arg("PlantParticleOracle")
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
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        for row in rows["shapes"].as_array().unwrap() {
            assert_eq!(row["enabled"].as_bool(), Some(true));
            let coordinates = row["position"].as_array().unwrap();
            let block = IVec3::new(
                coordinates[0].as_i64().unwrap() as i32,
                coordinates[1].as_i64().unwrap() as i32,
                coordinates[2].as_i64().unwrap() as i32,
            );
            let actual: Vec<_> = plant_fragments(block).collect();
            let expected = row["fragments"].as_array().unwrap();
            assert_eq!(actual.len(), expected.len(), "{row}");
            for ((position, velocity), values) in actual.iter().zip(expected) {
                for (actual, expected) in position
                    .to_array()
                    .into_iter()
                    .chain(velocity.to_array())
                    .zip(values.as_array().unwrap())
                {
                    assert_eq!(actual.to_bits(), expected.as_i64().unwrap() as u64, "{row}");
                }
            }
        }
        assert_eq!(rows["physics"].as_array().unwrap().len(), 200);
        for row in rows["physics"].as_array().unwrap() {
            let mut rng = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let mut particle = Particle::new_inner(
                DVec3::new(1.25, 3.5, -2.25),
                DVec3::new(0.1, 0.15, -0.05),
                ParticleVisual::Plant(PlantParticleKind::Fern, IVec3::ZERO),
                true,
                &mut rng,
            );
            assert_eq!(
                particle.lifetime,
                row["lifetime"].as_i64().unwrap() as i32,
                "{row}"
            );
            for (actual, field) in [
                (particle.size, "size_bits"),
                (particle.uv[0], "u_bits"),
                (particle.uv[1], "v_bits"),
            ] {
                assert_eq!(
                    actual.to_bits(),
                    row[field].as_i64().unwrap() as i32 as u32,
                    "{row}"
                );
            }
            let floor = CollisionShape::new(vec![BoundingBox {
                min: DVec3::new(-100.0, 2.0, -100.0),
                max: DVec3::new(100.0, 3.0, 100.0),
            }]);
            for (index, state) in row["states"].as_array().unwrap().iter().enumerate() {
                let alive = if index == 0 {
                    true
                } else {
                    particle.tick_with(|bounds, velocity| {
                        if row["floor"].as_bool().unwrap() {
                            resolve_movement(
                                bounds,
                                velocity,
                                false,
                                0.0,
                                std::slice::from_ref(&floor),
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
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn free_particle_physics_matches_java() {
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
        let source = temp.path().join("FoodParticlePhysicsOracle.java");
        let output = temp.path().join("particle.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/FoodParticlePhysicsOracle.java"),
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
                .arg("FoodParticlePhysicsOracle")
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
            let mut rng = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let mut particle = Particle::new(spawn(0), &mut rng);
            assert_eq!(
                particle.lifetime,
                row["lifetime"].as_i64().unwrap() as i32,
                "{row}"
            );
            for (actual, field) in [
                (particle.size, "size_bits"),
                (particle.uv[0], "u_bits"),
                (particle.uv[1], "v_bits"),
            ] {
                assert_eq!(
                    actual.to_bits(),
                    row[field].as_i64().unwrap() as i32 as u32,
                    "{row}"
                );
            }
            let floor = CollisionShape::new(vec![BoundingBox {
                min: DVec3::new(-100.0, 2.0, -100.0),
                max: DVec3::new(100.0, 3.0, 100.0),
            }]);
            for (index, state) in row["states"].as_array().unwrap().iter().enumerate() {
                let alive = if index == 0 {
                    true
                } else {
                    particle.tick_with(|bounds, velocity| {
                        if row["floor"].as_bool().unwrap() {
                            resolve_movement(
                                bounds,
                                velocity,
                                false,
                                0.0,
                                std::slice::from_ref(&floor),
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
            }
        }
    }
}
