//! Client-only random display ticks and particles for bubble columns.

use std::collections::{HashMap, VecDeque};

use glam::{DVec3, IVec3};
use native_core::random::LegacyRandom;
use native_session::{
    ConsumeSound, ConsumeSoundCategory, ConsumeSoundInfo, SessionWorld, WorldSnapshot,
};
use native_world::{collision::Aabb, fluid::FluidState, movement::BoundingBox};

const LIMIT: usize = 4096;
const MAX_TICKS_PER_UPDATE: u64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug)]
struct Spawn {
    position: DVec3,
    velocity: DVec3,
    kind: Kind,
}

/// Implements BubbleColumnBlock.animateTick. This random source belongs to
/// the client animation tick and never changes the server's simulation RNG.
fn animate_column(
    position: IVec3,
    drag_down: bool,
    random: &mut LegacyRandom,
    output: &mut Vec<Spawn>,
    sounds: &mut Vec<ConsumeSound>,
) {
    let base = position.as_dvec3();
    if drag_down {
        output.push(Spawn {
            position: base + DVec3::new(0.5, 0.8, 0.0),
            velocity: DVec3::ZERO,
            kind: Kind::Down,
        });
    } else {
        output.push(Spawn {
            position: base + DVec3::new(0.5, 0.0, 0.5),
            velocity: DVec3::new(0.0, 0.04, 0.0),
            kind: Kind::Up,
        });
        output.push(Spawn {
            position: base
                + DVec3::new(
                    f64::from(random.next_float()),
                    f64::from(random.next_float()),
                    f64::from(random.next_float()),
                ),
            velocity: DVec3::new(0.0, 0.04, 0.0),
            kind: Kind::Up,
        });
    }
    if random.next_int_bound(200) == Some(0) {
        sounds.push(ConsumeSound {
            position: base,
            info: ConsumeSoundInfo {
                event: if drag_down {
                    "block.bubble_column.whirlpool_ambient"
                } else {
                    "block.bubble_column.upwards_ambient"
                }
                .into(),
                volume: 0.2_f32 + random.next_float() * 0.2_f32,
                pitch: 0.9_f32 + random.next_float() * 0.15_f32,
                category: ConsumeSoundCategory::Block,
            },
        });
    }
}

/// Advance the separate visual random stream used by ordinary water's
/// animateTick. Underwater particles are not emitted here.
fn water_animation(fluid: FluidState, random: &mut LegacyRandom) {
    if !fluid.source && !fluid.falling {
        if random.next_int_bound(64) == Some(0) {
            random.next_float();
            random.next_float();
        }
    } else if random.next_int_bound(10) == Some(0) {
        random.next_double();
        random.next_double();
        random.next_double();
    }
}

fn sampled_position(center: IVec3, range: i32, random: &mut LegacyRandom) -> IVec3 {
    let mut offset =
        || random.next_int_bound(range).unwrap() - random.next_int_bound(range).unwrap();
    IVec3::new(
        center.x.wrapping_add(offset()),
        center.y.wrapping_add(offset()),
        center.z.wrapping_add(offset()),
    )
}

fn sample(
    world: &SessionWorld,
    center: IVec3,
    coordinates: &mut LegacyRandom,
    block_random: &mut LegacyRandom,
    output: &mut Vec<Spawn>,
    sounds: &mut Vec<ConsumeSound>,
) {
    let SessionWorld::Loaded(world) = world else {
        return;
    };
    for _ in 0..667 {
        for range in [16, 32] {
            let position = sampled_position(center, range, coordinates);
            let Some(state) = world
                .block_id(position.x, position.y, position.z)
                .and_then(|id| world.state(id))
            else {
                continue;
            };
            if state.name == "minecraft:bubble_column" && state.properties.len() == 1 {
                match state.properties.get("drag").map(String::as_str) {
                    Some("true") => animate_column(position, true, block_random, output, sounds),
                    Some("false") => animate_column(position, false, block_random, output, sounds),
                    _ => {}
                }
            }
            if let Ok(Some(fluid)) = FluidState::from_block(state) {
                if fluid.kind == native_world::fluid::FluidKind::Water {
                    water_animation(fluid, block_random);
                }
                // ClientLevel makes this draw for a fluid with a drip particle.
                coordinates.next_int_bound(10);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AmbientParticle {
    pub previous: DVec3,
    pub position: DVec3,
    velocity: DVec3,
    bounds: BoundingBox,
    pub size: f32,
    pub sprite: usize,
    pub kind: Kind,
    age: i32,
    lifetime: i32,
    angle: f32,
    stopped: bool,
    on_ground: bool,
}

impl AmbientParticle {
    fn new(spawn: Spawn, sprite: usize, random: &mut LegacyRandom) -> Self {
        let _initial = (4.0_f32 / (random.next_float() * 0.9_f32 + 0.1_f32)) as i32;
        let mut size = 0.1_f32 * (random.next_float() * 0.5_f32 + 0.5_f32) * 2.0_f32;
        let (velocity, lifetime) = match spawn.kind {
            Kind::Up => {
                size *= random.next_float() * 0.6_f32 + 0.2_f32;
                let mut component = |v: f64| {
                    v * f64::from(0.2_f32)
                        + f64::from((random.next_float() * 2.0_f32 - 1.0_f32) * 0.02_f32)
                };
                let velocity = DVec3::new(
                    component(spawn.velocity.x),
                    component(spawn.velocity.y),
                    component(spawn.velocity.z),
                );
                (
                    velocity,
                    (40.0 / (f64::from(random.next_float()) * 0.8 + 0.2)) as i32,
                )
            }
            Kind::Down => {
                let lifetime = (random.next_float() * 60.0_f32) as i32 + 30;
                size *= random.next_float() * 0.6_f32 + 0.2_f32;
                (DVec3::new(0.0, -0.05, 0.0), lifetime)
            }
        };
        let old_half = f64::from(0.1_f32);
        let width = f64::from(0.02_f32);
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
            kind: spawn.kind,
            age: 0,
            lifetime,
            angle: 0.0,
            stopped: false,
            on_ground: false,
        }
    }

    fn tick_with(
        &mut self,
        mut resolve: impl FnMut(BoundingBox, DVec3) -> DVec3,
        mut water: impl FnMut(DVec3) -> bool,
    ) -> bool {
        self.previous = self.position;
        let age = self.age;
        self.age += 1;
        if age >= self.lifetime {
            return false;
        }
        if self.kind == Kind::Up {
            self.velocity.y -= 0.04 * f64::from(-0.125_f32);
        } else {
            let cosine = native_session::physics::java_sin(f64::from(self.angle), true);
            let sine = native_session::physics::java_sin(f64::from(self.angle), false);
            self.velocity.x = (self.velocity.x + f64::from(0.6_f32 * cosine)) * 0.07;
            self.velocity.z = (self.velocity.z + f64::from(0.6_f32 * sine)) * 0.07;
        }
        if !self.stopped {
            let requested = self.velocity;
            let actual = if self.kind == Kind::Down {
                requested
            } else {
                resolve(self.bounds, requested)
            };
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
        if self.kind == Kind::Up {
            self.velocity *= f64::from(0.85_f32);
            if self.on_ground {
                self.velocity.x *= f64::from(0.7_f32);
                self.velocity.z *= f64::from(0.7_f32);
            }
        } else {
            self.angle += 0.08_f32;
        }
        water(self.position) && !(self.kind == Kind::Down && self.on_ground)
    }
}

pub(crate) struct AmbientBubbles {
    tick: Option<u64>,
    coordinates: LegacyRandom,
    block_random: LegacyRandom,
    sprite_random: LegacyRandom,
    particle_random: LegacyRandom,
    particles: VecDeque<AmbientParticle>,
    shapes: HashMap<u32, Option<Vec<Aabb>>>,
}

impl Default for AmbientBubbles {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |v| v.as_nanos() as i64);
        Self {
            tick: None,
            coordinates: LegacyRandom::new(seed),
            block_random: LegacyRandom::new(seed.wrapping_add(1)),
            sprite_random: LegacyRandom::new(seed.wrapping_add(2)),
            particle_random: LegacyRandom::new(seed.wrapping_add(3)),
            particles: VecDeque::new(),
            shapes: HashMap::new(),
        }
    }
}

impl AmbientBubbles {
    pub fn update(
        &mut self,
        snapshot: &WorldSnapshot,
        up_sprites: usize,
        down_sprites: usize,
    ) -> Vec<ConsumeSound> {
        let target = snapshot.tick;
        let mut tick = self.tick.unwrap_or(target.saturating_sub(1));
        if target < tick {
            self.particles.clear();
            self.shapes.clear();
            tick = target;
        }
        if target.saturating_sub(tick) > 100 {
            self.particles.clear();
            tick = target - 100;
        }
        let end = target.min(tick.saturating_add(MAX_TICKS_PER_UPDATE));
        let mut sounds = Vec::new();
        while tick < end {
            tick += 1;
            self.particles.retain_mut(|particle| {
                particle.tick_with(
                    |bounds, movement| {
                        crate::consume_particles::collide(
                            &snapshot.world,
                            &mut self.shapes,
                            bounds,
                            movement,
                        )
                    },
                    |position| crate::bubble_particles::water_at(&snapshot.world, position),
                )
            });
            let absolute = snapshot.origin.as_dvec3() + snapshot.player.position.as_dvec3();
            if !absolute.is_finite()
                || absolute.min_element() < f64::from(i32::MIN)
                || absolute.max_element() > f64::from(i32::MAX)
            {
                continue;
            }
            let mut spawns = Vec::new();
            let mut animation_random = LegacyRandom::new(self.block_random.next_long());
            sample(
                &snapshot.world,
                absolute.floor().as_ivec3(),
                &mut self.coordinates,
                &mut animation_random,
                &mut spawns,
                &mut sounds,
            );
            for spawn in spawns {
                let count = if spawn.kind == Kind::Up {
                    up_sprites
                } else {
                    down_sprites
                };
                if count == 0 {
                    continue;
                }
                if self.particles.len() == LIMIT {
                    self.particles.pop_front();
                }
                let sprite = self
                    .sprite_random
                    .next_int_bound(count as i32)
                    .expect("nonempty sprites") as usize;
                let mut random = LegacyRandom::new(self.particle_random.next_long());
                self.particles
                    .push_back(AmbientParticle::new(spawn, sprite, &mut random));
            }
        }
        self.tick = Some(tick);
        sounds
    }

    pub fn particles(&self) -> impl Iterator<Item = &AmbientParticle> {
        self.particles.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_is_seeded_and_uses_bounded_client_events() {
        for drag in [false, true] {
            let mut first = LegacyRandom::new(123);
            let mut second = first.clone();
            let mut a = Vec::new();
            let mut b = Vec::new();
            let mut a_sounds = Vec::new();
            let mut b_sounds = Vec::new();
            for _ in 0..1000 {
                animate_column(
                    IVec3::new(-8, 10, 17),
                    drag,
                    &mut first,
                    &mut a,
                    &mut a_sounds,
                );
                animate_column(
                    IVec3::new(-8, 10, 17),
                    drag,
                    &mut second,
                    &mut b,
                    &mut b_sounds,
                );
            }
            assert_eq!(a.len(), if drag { 1000 } else { 2000 });
            for (a, b) in a.iter().zip(&b) {
                assert_eq!(a.position, b.position);
                assert_eq!(a.velocity, b.velocity);
                assert_eq!(a.kind, b.kind);
            }
            assert_eq!(a_sounds, b_sounds);
            assert!(!a_sounds.is_empty());
            assert_eq!(first.next_int(), second.next_int());
        }
        assert!(MAX_TICKS_PER_UPDATE < 100);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn ambient_callbacks_and_particles_match_java() {
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
        let source = temp.path().join("AmbientBubbleOracle.java");
        let output = temp.path().join("ambient.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/AmbientBubbleOracle.java"),
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
                .arg("AmbientBubbleOracle")
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
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        for row in data["positions"].as_array().unwrap() {
            let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let positions = row["positions"].as_array().unwrap();
            assert_eq!(positions.len(), 1334);
            for (index, expected) in positions.iter().enumerate() {
                let actual = sampled_position(
                    IVec3::new(-123, 70, 456),
                    if index % 2 == 0 { 16 } else { 32 },
                    &mut random,
                );
                for (actual, expected) in actual.to_array().iter().zip(expected.as_array().unwrap())
                {
                    assert_eq!(i64::from(*actual), expected.as_i64().unwrap());
                }
            }
            assert_eq!(random.next_int(), row["next"].as_i64().unwrap() as i32);
        }
        let rows = data["animate"].as_array().unwrap();
        assert_eq!(rows.len(), 2000);
        for row in rows {
            let drag = row["drag"].as_bool().unwrap();
            let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let mut spawns = Vec::new();
            let mut sounds = Vec::new();
            animate_column(
                IVec3::new(-8, 10, 17),
                drag,
                &mut random,
                &mut spawns,
                &mut sounds,
            );
            let events = row["events"].as_array().unwrap();
            assert_eq!(events.len(), spawns.len() + sounds.len(), "{row}");
            for (spawn, expected) in spawns.iter().zip(events) {
                assert_eq!(
                    expected["kind"].as_str().unwrap(),
                    if spawn.kind == Kind::Up { "up" } else { "down" }
                );
                let number = |name: &str| f64::from_bits(expected[name].as_i64().unwrap() as u64);
                assert_eq!(
                    spawn.position,
                    DVec3::new(number("x"), number("y"), number("z")),
                    "{row}"
                );
                assert_eq!(
                    spawn.velocity,
                    DVec3::new(number("vx"), number("vy"), number("vz")),
                    "{row}"
                );
            }
            if let Some(sound) = sounds.first() {
                let expected = events.last().unwrap();
                assert_eq!(
                    format!("minecraft:{}", sound.info.event),
                    expected["sound"].as_str().unwrap()
                );
                assert_eq!(
                    sound.info.volume.to_bits(),
                    (expected["volume"].as_f64().unwrap() as f32).to_bits()
                );
                assert_eq!(
                    sound.info.pitch.to_bits(),
                    (expected["pitch"].as_f64().unwrap() as f32).to_bits()
                );
                assert_eq!(sound.position, DVec3::new(-8.0, 10.0, 17.0));
                assert_eq!(sound.info.category, ConsumeSoundCategory::Block);
            }
            assert_eq!(
                random.next_int(),
                row["next"].as_i64().unwrap() as i32,
                "{row}"
            );
        }
        let ceiling = CollisionShape::new(vec![BoundingBox {
            min: DVec3::new(-100.0, 4.0, -100.0),
            max: DVec3::new(100.0, 5.0, 100.0),
        }]);
        let rows = data["particles"].as_array().unwrap();
        assert_eq!(rows.len(), 600);
        for row in rows {
            let kind = if row["up"].as_bool().unwrap() {
                Kind::Up
            } else {
                Kind::Down
            };
            let mode = row["mode"].as_u64().unwrap();
            let spawn = Spawn {
                position: DVec3::new(1.25, 3.5, -2.25),
                velocity: DVec3::new(0.0, 0.04, 0.0),
                kind,
            };
            let mut particle = AmbientParticle::new(
                spawn,
                0,
                &mut LegacyRandom::new(row["seed"].as_i64().unwrap()),
            );
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
                                    std::slice::from_ref(&ceiling),
                                )
                            } else {
                                movement
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
                    let reference = f64::from_bits(expected.as_i64().unwrap() as u64);
                    assert!(
                        (actual - reference).abs() < 1.0e-6,
                        "kind={kind:?} mode={mode} tick={index} expected={reference} actual={actual}"
                    );
                }
                assert_eq!(particle.age, values[6].as_i64().unwrap() as i32, "{row}");
                assert_eq!(
                    particle.lifetime,
                    values[7].as_i64().unwrap() as i32,
                    "{row}"
                );
                assert_eq!(alive, values[8].as_bool().unwrap(), "{row}");
            }
        }
    }
}
