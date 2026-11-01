//! Data-pack driven per-tick enchantment particle and sound effects.

use anyhow::{Context, Result, bail, ensure};
use native_core::random::LegacyRandom;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use super::{
    EquipmentSlot, load_single_tag,
    location::{LocationContext, Predicate, check_keys, finite_f32, id, object},
};
use crate::packs::{ResourceId, ResourceProvider};

#[derive(Clone, Debug, PartialEq)]
enum FloatValue {
    Constant(f32),
    Uniform(f32, f32),
}
impl FloatValue {
    fn parse(value: &Value) -> Result<Self> {
        if value.is_number() {
            return Ok(Self::Constant(finite_f32(value)?));
        }
        let value = object(value, "float provider")?;
        check_keys(value, &["type", "min_inclusive", "max_exclusive"])?;
        let kind = id(
            value
                .get("type")
                .context("float provider type is missing")?,
            "float provider",
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "uniform",
            "unsupported float provider"
        );
        let min = finite_f32(value.get("min_inclusive").context("minimum is missing")?)?;
        let max = finite_f32(value.get("max_exclusive").context("maximum is missing")?)?;
        ensure!(min <= max, "invalid uniform range");
        Ok(Self::Uniform(min, max))
    }
    fn sample(&self, random: &mut LegacyRandom) -> f32 {
        match self {
            Self::Constant(value) => *value,
            Self::Uniform(min, max) => min + random.next_float() * (max - min),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum PositionKind {
    Entity,
    Box,
}
#[derive(Clone, Debug, PartialEq)]
struct PositionSource {
    kind: PositionKind,
    offset: f32,
    scale: f32,
}
impl PositionSource {
    fn parse(value: &Value) -> Result<Self> {
        let value = object(value, "particle position source")?;
        check_keys(value, &["type", "offset", "scale"])?;
        let kind = id(
            value
                .get("type")
                .context("particle position type is missing")?,
            "particle position type",
        )?;
        ensure!(
            kind.namespace == "minecraft",
            "unsupported particle position namespace"
        );
        let kind = match kind.path.as_str() {
            "entity_position" => PositionKind::Entity,
            "in_bounding_box" => PositionKind::Box,
            _ => bail!("unsupported particle position type"),
        };
        let offset = value
            .get("offset")
            .map(finite_f32)
            .transpose()?
            .unwrap_or(0.0);
        let scale = value
            .get("scale")
            .map(finite_f32)
            .transpose()?
            .unwrap_or(1.0);
        ensure!(
            !matches!(kind, PositionKind::Entity) || scale == 1.0,
            "entity position cannot be scaled"
        );
        Ok(Self {
            kind,
            offset,
            scale,
        })
    }
    fn coordinate(&self, origin: f64, center: f64, size: f32, random: &mut LegacyRandom) -> f64 {
        let value = match self.kind {
            PositionKind::Entity => origin,
            PositionKind::Box => {
                center + (random.next_double() - 0.5) * f64::from(size * self.scale)
            }
        };
        value + f64::from(self.offset)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct VelocitySource {
    movement_scale: f32,
    base: FloatValue,
}
impl VelocitySource {
    fn parse(value: &Value) -> Result<Self> {
        let value = object(value, "particle velocity source")?;
        check_keys(value, &["movement_scale", "base"])?;
        Ok(Self {
            movement_scale: value
                .get("movement_scale")
                .map(finite_f32)
                .transpose()?
                .unwrap_or(0.0),
            base: value
                .get("base")
                .map(FloatValue::parse)
                .transpose()?
                .unwrap_or(FloatValue::Constant(0.0)),
        })
    }
    fn velocity(&self, movement: f64, random: &mut LegacyRandom) -> f64 {
        movement * f64::from(self.movement_scale) + f64::from(self.base.sample(random))
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Effect {
    Particle {
        id: ResourceId,
        horizontal: PositionSource,
        vertical: PositionSource,
        horizontal_velocity: VelocitySource,
        vertical_velocity: VelocitySource,
        speed: FloatValue,
    },
    Sound {
        sounds: Vec<ResourceId>,
        volume: FloatValue,
        pitch: FloatValue,
    },
}

impl Effect {
    fn parse(value: &Value) -> Result<Self> {
        let value = object(value, "tick effect")?;
        let kind = id(
            value.get("type").context("tick effect type missing")?,
            "effect type",
        )?;
        ensure!(
            kind.namespace == "minecraft",
            "unsupported tick effect namespace"
        );
        match kind.path.as_str() {
            "spawn_particles" => {
                check_keys(
                    value,
                    &[
                        "type",
                        "particle",
                        "horizontal_position",
                        "vertical_position",
                        "horizontal_velocity",
                        "vertical_velocity",
                        "speed",
                    ],
                )?;
                let particle = object(
                    value.get("particle").context("particle missing")?,
                    "particle",
                )?;
                check_keys(particle, &["type"])?;
                Ok(Self::Particle {
                    id: id(
                        particle.get("type").context("particle type missing")?,
                        "particle type",
                    )?,
                    horizontal: PositionSource::parse(
                        value
                            .get("horizontal_position")
                            .context("horizontal position missing")?,
                    )?,
                    vertical: PositionSource::parse(
                        value
                            .get("vertical_position")
                            .context("vertical position missing")?,
                    )?,
                    horizontal_velocity: VelocitySource::parse(
                        value
                            .get("horizontal_velocity")
                            .context("horizontal velocity missing")?,
                    )?,
                    vertical_velocity: VelocitySource::parse(
                        value
                            .get("vertical_velocity")
                            .context("vertical velocity missing")?,
                    )?,
                    speed: FloatValue::parse(
                        value.get("speed").context("particle speed missing")?,
                    )?,
                })
            }
            "play_sound" => {
                check_keys(value, &["type", "sound", "volume", "pitch"])?;
                let sounds = value.get("sound").context("sound missing")?;
                let sounds = if let Some(values) = sounds.as_array() {
                    values
                        .iter()
                        .map(|v| id(v, "sound"))
                        .collect::<Result<Vec<_>>>()?
                } else {
                    vec![id(sounds, "sound")?]
                };
                ensure!(!sounds.is_empty(), "empty sound list");
                Ok(Self::Sound {
                    sounds,
                    volume: FloatValue::parse(value.get("volume").context("volume missing")?)?,
                    pitch: FloatValue::parse(value.get("pitch").context("pitch missing")?)?,
                })
            }
            _ => bail!("unsupported tick effect {}", kind.path),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Conditional {
    predicate: Predicate,
    effect: Effect,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TickAction {
    Particle {
        id: ResourceId,
        position: [f64; 3],
        velocity: [f64; 3],
        speed: f32,
    },
    Sound {
        id: ResourceId,
        volume: f32,
        pitch: f32,
    },
}

pub struct TickContext<'a> {
    pub location: LocationContext<'a>,
    pub position: [f64; 3],
    pub width: f32,
    pub height: f32,
    pub silent: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TickEnchantment {
    slots: Vec<String>,
    effects: Vec<Conditional>,
    tags: BTreeMap<ResourceId, BTreeSet<String>>,
}
impl TickEnchantment {
    pub fn load(provider: &mut impl ResourceProvider, value: &Value) -> Result<Self> {
        let definition = super::definition(value)?;
        let mut entries = None;
        if let Some(effects) = value.get("effects") {
            let effects = object(effects, "enchantment effects")?;
            for key in ["minecraft:tick", "tick", ":tick"] {
                if let Some(value) = effects.get(key) {
                    ensure!(entries.is_none(), "duplicate tick effect component");
                    entries = Some(value);
                }
            }
        }
        let mut effects = Vec::new();
        if let Some(entries) = entries {
            let entries = entries.as_array().context("tick effects are not a list")?;
            ensure!(entries.len() <= 256, "too many tick effects");
            for entry in entries {
                let entry = object(entry, "conditional tick effect")?;
                check_keys(entry, &["effect", "requirements"])?;
                effects.push(Conditional {
                    effect: Effect::parse(entry.get("effect").context("tick effect missing")?)?,
                    predicate: entry
                        .get("requirements")
                        .map(|v| Predicate::parse(v, 0))
                        .transpose()?
                        .unwrap_or(Predicate::Always),
                });
            }
        }
        let mut ids = BTreeSet::new();
        for effect in &effects {
            effect.predicate.tags(&mut ids);
        }
        let tags = ids
            .into_iter()
            .map(|id| Ok((id.clone(), load_single_tag(provider, "block", &id)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            slots: definition.slots,
            effects,
            tags,
        })
    }
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
    /// Callers must stage RNGs until all returned actions can be committed.
    pub fn evaluate(
        &self,
        slot: EquipmentSlot,
        context: &TickContext<'_>,
        level_random: &mut LegacyRandom,
        entity_random: &mut LegacyRandom,
    ) -> Result<Vec<TickAction>> {
        ensure!(
            (1..=255).contains(&context.location.level),
            "invalid enchantment level"
        );
        if !self.slots.iter().any(|group| slot.matches(group)) {
            return Ok(Vec::new());
        }
        let mut actions = Vec::new();
        for conditional in &self.effects {
            if !conditional
                .predicate
                .matches(&context.location, &self.tags, &mut || {
                    level_random.next_float()
                })?
            {
                continue;
            }
            match &conditional.effect {
                Effect::Sound {
                    sounds,
                    volume,
                    pitch,
                } => {
                    if !context.silent {
                        let index = (context.location.level as usize - 1).min(sounds.len() - 1);
                        actions.push(TickAction::Sound {
                            id: sounds[index].clone(),
                            volume: volume.sample(entity_random),
                            pitch: pitch.sample(entity_random),
                        });
                    }
                }
                Effect::Particle {
                    id,
                    horizontal,
                    vertical,
                    horizontal_velocity,
                    vertical_velocity,
                    speed,
                } => {
                    let [x, y, z] = context.position;
                    let [dx, dy, dz] = context.location.known_movement;
                    let position = [
                        horizontal.coordinate(x, x, context.width, entity_random),
                        vertical.coordinate(
                            y,
                            y + f64::from(context.height / 2.0),
                            context.height,
                            entity_random,
                        ),
                        horizontal.coordinate(z, z, context.width, entity_random),
                    ];
                    let velocity = [
                        horizontal_velocity.velocity(dx, entity_random),
                        vertical_velocity.velocity(dy, entity_random),
                        horizontal_velocity.velocity(dz, entity_random),
                    ];
                    actions.push(TickAction::Particle {
                        id: id.clone(),
                        position,
                        velocity,
                        speed: speed.sample(entity_random),
                    });
                }
            }
        }
        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the installed Minecraft 26.3 assets"]
    fn installed_soul_speed_tick_effects_use_conditions_and_separate_rngs() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut resources = crate::packs::ResourceStack::open(&game).unwrap();
        let registry = super::super::OxygenEnchantments::load(&mut resources).unwrap();
        let effect = registry
            .tick_effects(&ResourceId::parse("minecraft:soul_speed").unwrap())
            .unwrap();
        let mut level_random = LegacyRandom::new(0);
        let mut entity_random = LegacyRandom::new(1);
        let context = TickContext {
            location: LocationContext {
                active: false,
                flying: false,
                on_ground: true,
                vehicle: false,
                movement_block: "minecraft:soul_sand",
                level: 3,
                tick_count: 5,
                known_movement: [0.2, 0.0, -0.1],
            },
            position: [8.5, 1.875, 8.5],
            width: 0.6,
            height: 1.8,
            silent: false,
        };
        let actions = effect
            .evaluate(
                EquipmentSlot::Feet,
                &context,
                &mut level_random,
                &mut entity_random,
            )
            .unwrap();
        assert!(
            matches!(actions.first(), Some(TickAction::Particle { id, .. }) if id.namespace == "minecraft" && id.path == "soul")
        );
        let mut level_random = LegacyRandom::new(0);
        let mut entity_random = LegacyRandom::new(1);
        let context = TickContext {
            location: LocationContext {
                tick_count: 4,
                ..context.location
            },
            ..context
        };
        assert!(
            effect
                .evaluate(
                    EquipmentSlot::Feet,
                    &context,
                    &mut level_random,
                    &mut entity_random
                )
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn soul_speed_tick_effects_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: Value =
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
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("TickEnchantmentOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../../tests/java/TickEnchantmentOracle.java"),
        )
        .unwrap();
        let mut cases = Vec::new();
        for level_seed in [0, 5120] {
            for entity_seed in [0, 1] {
                for level in [1, 3] {
                    for tick in [4, 5] {
                        for ground in [false, true] {
                            for flying in [false, true] {
                                for silent in [false, true] {
                                    for soul in [false, true] {
                                        for vx in [0.0, 0.0000001, 0.000001, 0.2] {
                                            cases.push(serde_json::json!({"level_seed":level_seed,"entity_seed":entity_seed,"level":level,"tick":tick,"ground":ground,"flying":flying,"silent":silent,"soul":soul,"vx":vx,"vz":0.0}));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("TickEnchantmentOracle")
                .arg(&input)
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
        let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(cases.len(), expected.len());
        let mut resources = crate::packs::ResourceStack::open(&game).unwrap();
        let registry = super::super::OxygenEnchantments::load(&mut resources).unwrap();
        let effect = registry
            .tick_effects(&ResourceId::parse("minecraft:soul_speed").unwrap())
            .unwrap();
        for (case, expected) in cases.iter().zip(expected) {
            let flag = |key: &str| case[key].as_bool().unwrap();
            let mut level_random = LegacyRandom::new(case["level_seed"].as_i64().unwrap());
            let mut entity_random = LegacyRandom::new(case["entity_seed"].as_i64().unwrap());
            let context = TickContext {
                location: LocationContext {
                    active: false,
                    flying: flag("flying"),
                    on_ground: flag("ground"),
                    vehicle: false,
                    movement_block: if flag("soul") {
                        "minecraft:soul_sand"
                    } else {
                        "minecraft:stone"
                    },
                    level: case["level"].as_u64().unwrap() as u32,
                    tick_count: case["tick"].as_i64().unwrap() as i32,
                    known_movement: [
                        case["vx"].as_f64().unwrap(),
                        0.0,
                        case["vz"].as_f64().unwrap(),
                    ],
                },
                position: [8.5, 1.875, 8.5],
                width: 0.6,
                height: 1.8,
                silent: flag("silent"),
            };
            let actual = effect
                .evaluate(
                    EquipmentSlot::Feet,
                    &context,
                    &mut level_random,
                    &mut entity_random,
                )
                .unwrap();
            let events = expected["events"].as_array().unwrap();
            assert_eq!(actual.len(), events.len(), "{case}");
            for (actual, expected) in actual.iter().zip(events) {
                match actual {
                    TickAction::Particle {
                        id,
                        position,
                        velocity,
                        speed,
                    } => {
                        assert_eq!(id.path, "soul");
                        assert_eq!(expected["kind"], "particle");
                        assert_eq!(expected["count"], 0);
                        for (a, b) in position
                            .iter()
                            .zip(expected["position"].as_array().unwrap())
                        {
                            assert_eq!(a.to_bits(), b.as_i64().unwrap() as u64, "{case}");
                        }
                        for (a, b) in velocity
                            .iter()
                            .zip(expected["velocity"].as_array().unwrap())
                        {
                            assert_eq!(a.to_bits(), b.as_i64().unwrap() as u64, "{case}");
                        }
                        assert_eq!(
                            f64::from(*speed),
                            expected["speed"].as_f64().unwrap(),
                            "{case}"
                        );
                    }
                    TickAction::Sound { id, volume, pitch } => {
                        assert_eq!(expected["kind"], "sound");
                        assert_eq!(
                            format!("{}:{}", id.namespace, id.path),
                            expected["id"].as_str().unwrap()
                        );
                        assert_eq!(
                            volume.to_bits(),
                            (expected["volume"].as_f64().unwrap() as f32).to_bits(),
                            "{case}"
                        );
                        assert_eq!(
                            pitch.to_bits(),
                            (expected["pitch"].as_f64().unwrap() as f32).to_bits(),
                            "{case}"
                        );
                    }
                }
            }
            assert_eq!(
                level_random.next_int(),
                expected["level_next"].as_i64().unwrap() as i32,
                "{case}"
            );
            assert_eq!(
                entity_random.next_int(),
                expected["entity_next"].as_i64().unwrap() as i32,
                "{case}"
            );
        }
    }
}
