//! Read and prepare 26.3 player inventory data without writing to the world.

use crate::{
    GameMode,
    inventory::{ItemStack, PlayerInventory, PlayerSlot, RecipeBookState},
};
use anyhow::{Context, Result, bail, ensure};
use glam::DVec3;
use native_persistence::nbt::{NamedTag, Tag, TagType};
use native_resources::{
    enchantments::{AttributeModifier, AttributeOperation},
    packs::ResourceId,
};

/// The entity fields that affect the first movement tick after loading.
/// Velocity is in blocks per tick, as in Java's `Motion` tag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedPlayerMotion {
    pub velocity: DVec3,
    pub on_ground: bool,
    pub fall_distance: f64,
}

/// Player fields captured at a completed simulation tick. Angles use Java's
/// degrees (yaw then pitch); position is absolute rather than chunk-relative.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerSaveState {
    pub position: DVec3,
    pub motion: SavedPlayerMotion,
    pub yaw: f32,
    pub pitch: f32,
    pub dimension: String,
    pub game_mode: GameMode,
    pub vitals: crate::survival::PlayerVitals,
    pub experience: crate::experience::Experience,
    pub score: i32,
}

/// Read the last-death GlobalPos without discarding unrecognized saved data.
pub fn read_last_death_location(player: &NamedTag) -> Result<Option<crate::lodestone::Target>> {
    unique_field(&player.value, "LastDeathLocation")?
        .map(|value| {
            crate::lodestone::Target::parse(value).context("unsupported last-death location")
        })
        .transpose()
}

pub(crate) fn numeric_double(tag: &Tag) -> Option<f64> {
    Some(match tag {
        Tag::Byte(value) => f64::from(*value),
        Tag::Short(value) => f64::from(*value),
        Tag::Int(value) => f64::from(*value),
        Tag::Long(value) => *value as f64,
        Tag::Float(value) => f64::from(*value),
        Tag::Double(value) => *value,
        _ => return None,
    })
}

pub(crate) fn numeric_int(tag: &Tag) -> Option<i32> {
    Some(match tag {
        Tag::Byte(value) => i32::from(*value),
        Tag::Short(value) => i32::from(*value),
        Tag::Int(value) => *value,
        Tag::Long(value) => *value as i32,
        Tag::Float(value) => java_floor(f64::from(*value)),
        Tag::Double(value) => java_floor(*value),
        _ => return None,
    })
}

fn numeric_float(tag: &Tag) -> Option<f32> {
    Some(match tag {
        Tag::Byte(value) => f32::from(*value),
        Tag::Short(value) => f32::from(*value),
        Tag::Int(value) => *value as f32,
        Tag::Long(value) => *value as f32,
        Tag::Float(value) => *value,
        Tag::Double(value) => *value as f32,
        _ => return None,
    })
}

pub(crate) fn text(tag: &Tag) -> Option<String> {
    match tag {
        Tag::String(value) => String::from_utf16(&value.0).ok(),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedAttribute {
    base: f64,
    modifiers: std::collections::BTreeMap<ResourceId, AttributeModifier>,
}

impl SavedAttribute {
    pub(crate) fn with_base(base: f64) -> Self {
        Self {
            base,
            modifiers: Default::default(),
        }
    }

    pub(crate) fn apply(&mut self, modifier: AttributeModifier) {
        self.modifiers.insert(modifier.id.clone(), modifier);
    }

    pub(crate) fn contains_modifier(&self, id: &ResourceId) -> bool {
        self.modifiers.contains_key(id)
    }

    pub(crate) fn clear_modifiers(&mut self) {
        self.modifiers.clear();
    }

    pub(crate) fn remove_modifier(&mut self, id: &ResourceId) {
        self.modifiers.remove(id);
    }

    pub(crate) fn set_base(&mut self, base: f64) {
        self.base = base;
    }

    pub(crate) fn modifier(&self, id: &ResourceId) -> Option<&AttributeModifier> {
        self.modifiers.get(id)
    }

    pub(crate) fn value(&self, minimum: f64, maximum: f64) -> Result<f64> {
        let mut value = self.base;
        for modifier in self
            .modifiers
            .values()
            .filter(|m| m.operation == AttributeOperation::AddValue)
        {
            value += modifier.amount;
        }
        let adjusted_base = value;
        for modifier in self
            .modifiers
            .values()
            .filter(|m| m.operation == AttributeOperation::AddMultipliedBase)
        {
            value += adjusted_base * modifier.amount;
        }
        for modifier in self
            .modifiers
            .values()
            .filter(|m| m.operation == AttributeOperation::AddMultipliedTotal)
        {
            value *= 1.0 + modifier.amount;
        }
        ensure!(value.is_finite(), "saved attribute is not finite");
        Ok(value.clamp(minimum, maximum))
    }

    /// Apply vanilla effect and sprint modifiers to the assembled movement attribute.
    pub fn movement_speed(
        &self,
        speed: Option<u8>,
        slowness: Option<u8>,
        sprinting: bool,
    ) -> Result<f32> {
        let mut attribute = self.clone();
        for (name, amplifier, amount) in [
            ("effect.speed", speed, f64::from(0.2_f32)),
            ("effect.slowness", slowness, f64::from(-0.15_f32)),
        ] {
            if let Some(amplifier) = amplifier {
                attribute.apply(AttributeModifier {
                    id: ResourceId::parse(&format!("minecraft:{name}"))?,
                    operation: AttributeOperation::AddMultipliedTotal,
                    amount: amount * (f64::from(amplifier) + 1.0),
                });
            }
        }
        if sprinting {
            attribute.apply(AttributeModifier {
                id: ResourceId::parse("minecraft:sprinting")?,
                operation: AttributeOperation::AddMultipliedTotal,
                amount: f64::from(0.3_f32),
            });
        }
        Ok(attribute.value(0.0, 1024.0)? as f32)
    }
}

fn saved_attribute(player: &NamedTag, name: &str, default: f64) -> Result<SavedAttribute> {
    let mut attribute_state = SavedAttribute::with_base(default);
    let Some(Tag::List { values, .. }) = player.value.get("attributes") else {
        return Ok(attribute_state);
    };
    for attribute in values {
        let Some(id) = attribute.get("id").and_then(text) else {
            continue;
        };
        if id.strip_prefix("minecraft:").unwrap_or(&id) != name {
            continue;
        }
        attribute_state.base = attribute
            .get("base")
            .and_then(numeric_double)
            .unwrap_or(0.0);
        if let Some(Tag::List { values, .. }) = attribute.get("modifiers") {
            for modifier in values {
                let id = modifier
                    .get("id")
                    .and_then(text)
                    .context("attribute modifier has no id")?;
                let id = ResourceId::parse(&id).context("invalid attribute modifier id")?;
                let amount = modifier
                    .get("amount")
                    .and_then(numeric_double)
                    .context("attribute modifier has no numeric amount")?;
                let operation = modifier
                    .get("operation")
                    .and_then(text)
                    .context("attribute modifier has no operation")?;
                let operation = AttributeOperation::parse(&operation)?;
                attribute_state.apply(AttributeModifier {
                    id,
                    amount,
                    operation,
                });
            }
        }
    }
    Ok(attribute_state)
}

fn saved_max_health(player: &NamedTag) -> Result<f32> {
    Ok(saved_attribute(player, "max_health", 20.0)?.value(1.0, 1024.0)? as f32)
}

/// After death Java copies attribute base values, but not persistent modifiers.
pub fn read_respawn_max_health(player: &NamedTag) -> Result<f32> {
    let mut attribute = saved_attribute(player, "max_health", 20.0)?;
    attribute.clear_modifiers();
    Ok(attribute.value(1.0, 1024.0)? as f32)
}

/// Oxygen bonus supplied by saved persistent attributes; equipment modifiers
/// are evaluated separately when the equipped items are available.
pub fn read_saved_oxygen_bonus(player: &NamedTag) -> Result<f64> {
    read_saved_oxygen_attribute(player)?.value(0.0, 1024.0)
}

/// Preserve the base and modifiers so equipment effects can be applied before clamping.
pub fn read_saved_oxygen_attribute(player: &NamedTag) -> Result<SavedAttribute> {
    saved_attribute(player, "oxygen_bonus", 0.0)
}

/// Preserve the player's luck base and persistent modifiers for chest loot.
pub fn read_saved_luck_attribute(player: &NamedTag) -> Result<SavedAttribute> {
    if let Some(effects) = unique_field(&player.value, "active_effects")? {
        let Tag::List {
            element_type,
            values,
        } = effects
        else {
            bail!("active effects are not a list")
        };
        ensure!(
            *element_type == TagType::Compound || values.is_empty(),
            "active effects are not compounds"
        );
        let mut seen = std::collections::BTreeSet::new();
        for effect in values {
            let id = unique_field(effect, "id")?
                .and_then(text)
                .context("effect id missing")?;
            let id = ResourceId::parse(&id)?;
            ensure!(
                id.namespace == "minecraft"
                    && matches!(
                        id.path.as_str(),
                        "speed"
                            | "slowness"
                            | "haste"
                            | "mining_fatigue"
                            | "strength"
                            | "instant_health"
                            | "instant_damage"
                            | "jump_boost"
                            | "nausea"
                            | "regeneration"
                            | "resistance"
                            | "fire_resistance"
                            | "water_breathing"
                            | "invisibility"
                            | "blindness"
                            | "night_vision"
                            | "hunger"
                            | "weakness"
                            | "poison"
                            | "wither"
                            | "health_boost"
                            | "absorption"
                            | "saturation"
                            | "glowing"
                            | "levitation"
                            | "luck"
                            | "unluck"
                            | "slow_falling"
                            | "conduit_power"
                            | "dolphins_grace"
                            | "bad_omen"
                            | "hero_of_the_village"
                            | "darkness"
                            | "trial_omen"
                            | "raid_omen"
                            | "wind_charged"
                            | "weaving"
                            | "oozing"
                            | "infested"
                            | "breath_of_the_nautilus"
                    ),
                "unknown status effect may affect luck"
            );
            ensure!(seen.insert(id.path.clone()), "duplicate status effect");
            if matches!(id.path.as_str(), "luck" | "unluck") {
                fn validate(effect: &Tag, depth: usize) -> Result<()> {
                    ensure!(depth <= 256, "luck effect nesting too deep");
                    let Tag::Compound(fields) = effect else {
                        bail!("invalid luck effect")
                    };
                    ensure!(
                        fields.iter().all(|(key, _)| [
                            "amplifier",
                            "duration",
                            "ambient",
                            "show_particles",
                            "show_icon",
                            "hidden_effect"
                        ]
                        .iter()
                        .any(|name| key.matches(name))
                            || (depth == 0 && key.matches("id"))),
                        "unknown luck effect data"
                    );
                    for name in ["ambient", "show_particles", "show_icon"] {
                        if let Some(value) = unique_field(effect, name)? {
                            ensure!(
                                matches!(value, Tag::Byte(0 | 1)),
                                "invalid luck effect flag"
                            );
                        }
                    }
                    if let Some(value) = unique_field(effect, "amplifier")? {
                        ensure!(
                            matches!(value, Tag::Byte(_)),
                            "invalid luck effect amplifier"
                        );
                    }
                    if let Some(value) = unique_field(effect, "duration")? {
                        ensure!(
                            matches!(value, Tag::Int(-1..)),
                            "invalid luck effect duration"
                        );
                    }
                    if let Some(hidden) = unique_field(effect, "hidden_effect")? {
                        validate(hidden, depth + 1)?;
                    }
                    Ok(())
                }
                validate(effect, 0)?;
            }
        }
    }
    if let Some(attributes) = unique_field(&player.value, "attributes")? {
        let Tag::List {
            element_type,
            values,
        } = attributes
        else {
            bail!("attributes are not a list")
        };
        ensure!(
            *element_type == TagType::Compound || values.is_empty(),
            "attributes are not compounds"
        );
        let mut count = 0;
        for attribute in values {
            let Some(id) = unique_field(attribute, "id")?.and_then(text) else {
                continue;
            };
            let id = ResourceId::parse(&id)?;
            if id.namespace != "minecraft" || id.path != "luck" {
                continue;
            }
            let Tag::Compound(fields) = attribute else {
                bail!("invalid luck attribute")
            };
            ensure!(
                fields.iter().all(|(key, _)| ["id", "base", "modifiers"]
                    .iter()
                    .any(|name| key.matches(name))),
                "unknown luck attribute data"
            );
            count += 1;
            ensure!(count == 1, "duplicate luck attribute");
            if let Some(base) = unique_field(attribute, "base")? {
                ensure!(
                    numeric_double(base).is_some_and(f64::is_finite),
                    "invalid luck base"
                );
            }
            if let Some(modifiers) = unique_field(attribute, "modifiers")? {
                let Tag::List {
                    element_type,
                    values,
                } = modifiers
                else {
                    bail!("invalid luck modifiers")
                };
                ensure!(
                    *element_type == TagType::Compound || values.is_empty(),
                    "invalid luck modifiers"
                );
                let mut ids = std::collections::BTreeSet::new();
                for modifier in values {
                    let Tag::Compound(fields) = modifier else {
                        bail!("invalid luck modifier")
                    };
                    ensure!(
                        fields.iter().all(|(key, _)| ["id", "amount", "operation"]
                            .iter()
                            .any(|name| key.matches(name))),
                        "unknown luck modifier data"
                    );
                    let id = unique_field(modifier, "id")?
                        .and_then(text)
                        .context("luck modifier id missing")?;
                    ensure!(
                        ids.insert(ResourceId::parse(&id)?),
                        "duplicate luck modifier"
                    );
                    ensure!(
                        unique_field(modifier, "amount")?
                            .and_then(numeric_double)
                            .is_some_and(f64::is_finite),
                        "invalid luck modifier amount"
                    );
                    let operation = unique_field(modifier, "operation")?
                        .and_then(text)
                        .context("luck modifier operation missing")?;
                    AttributeOperation::parse(&operation)?;
                }
            }
        }
    }
    let attribute = saved_attribute(player, "luck", 0.0)?;
    for id in ["minecraft:effect.luck", "minecraft:effect.unluck"] {
        ensure!(
            !attribute.contains_modifier(&ResourceId::parse(id)?),
            "persistent luck effect modifier cannot be reconciled"
        );
    }
    Ok(attribute)
}

/// Persistent bounciness is combined with transient equipped-item modifiers.
pub fn read_saved_bounciness_attribute(player: &NamedTag) -> Result<SavedAttribute> {
    saved_attribute(player, "bounciness", 0.0)
}

/// Persistent jump strength; transient equipment modifiers are applied later.
pub fn read_saved_jump_strength_attribute(player: &NamedTag) -> Result<SavedAttribute> {
    saved_attribute(player, "jump_strength", f64::from(0.42_f32))
}

pub fn read_saved_gravity_attribute(player: &NamedTag) -> Result<SavedAttribute> {
    saved_attribute(player, "gravity", 0.08)
}

pub fn read_saved_water_movement_efficiency_attribute(player: &NamedTag) -> Result<SavedAttribute> {
    saved_attribute(player, "water_movement_efficiency", 0.0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedMiningAttributes {
    pub efficiency: SavedAttribute,
    pub block_break_speed: SavedAttribute,
    pub submerged_speed: SavedAttribute,
}

impl Default for SavedMiningAttributes {
    fn default() -> Self {
        Self {
            efficiency: SavedAttribute::with_base(0.0),
            block_break_speed: SavedAttribute::with_base(1.0),
            submerged_speed: SavedAttribute::with_base(0.2),
        }
    }
}

impl SavedMiningAttributes {
    pub(crate) fn values(&self) -> Result<[f64; 3]> {
        Ok([
            self.efficiency.value(0.0, 1024.0)?,
            self.block_break_speed.value(0.0, 1024.0)?,
            self.submerged_speed.value(0.0, 20.0)?,
        ])
    }
    pub(crate) fn clear_modifiers(&mut self) {
        self.efficiency.clear_modifiers();
        self.block_break_speed.clear_modifiers();
        self.submerged_speed.clear_modifiers();
    }
}

pub fn read_saved_mining_attributes(player: &NamedTag) -> Result<SavedMiningAttributes> {
    Ok(SavedMiningAttributes {
        efficiency: saved_attribute(player, "mining_efficiency", 0.0)?,
        block_break_speed: saved_attribute(player, "block_break_speed", 1.0)?,
        submerged_speed: saved_attribute(player, "submerged_mining_speed", 0.2)?,
    })
}

pub fn read_saved_no_gravity(player: &NamedTag) -> bool {
    player
        .value
        .get("NoGravity")
        .and_then(numeric_byte)
        .is_some_and(|value| value != 0)
}

pub fn read_saved_movement_speed_attribute(player: &NamedTag) -> Result<SavedAttribute> {
    saved_attribute(player, "movement_speed", f64::from(0.1_f32))
}

/// Player.load overwrites this attribute's saved base with abilities.walkSpeed.
pub fn read_loaded_movement_speed_attribute(player: &NamedTag) -> Result<SavedAttribute> {
    let mut attribute = read_saved_movement_speed_attribute(player)?;
    attribute.set_base(f64::from(read_saved_walking_speed(player)?));
    Ok(attribute)
}

/// Preserve persistent armor and toughness modifiers for equipment changes.
pub fn read_saved_armor_attributes(player: &NamedTag) -> Result<(SavedAttribute, SavedAttribute)> {
    Ok((
        saved_attribute(player, "armor", 0.0)?,
        saved_attribute(player, "armor_toughness", 0.0)?,
    ))
}

/// Persistent values are combined with transient status and equipment modifiers.
pub fn read_saved_fall_attributes(player: &NamedTag) -> Result<(SavedAttribute, SavedAttribute)> {
    Ok((
        saved_attribute(player, "safe_fall_distance", 3.0)?,
        saved_attribute(player, "fall_damage_multiplier", 1.0)?,
    ))
}

fn effect_duration(tag: &Tag, depth: usize) -> Result<crate::survival::EffectDuration> {
    ensure!(depth <= 256, "status effect nesting is too deep");
    ensure!(
        matches!(tag, Tag::Compound(_)),
        "status effect is not a compound"
    );
    let number = |name: &str, default: i32| -> Result<i32> {
        match tag.get(name) {
            None => Ok(default),
            Some(value) => value
                .as_java_int()
                .with_context(|| format!("invalid {name}")),
        }
    };
    // Java's UNSIGNED_BYTE codec reinterprets a signed NBT byte as 0..255.
    let amplifier = number("amplifier", 0)? as u8;
    let duration = number("duration", 0)?;
    let hidden = tag
        .get("hidden_effect")
        .map(|hidden| effect_duration(hidden, depth + 1).map(Box::new))
        .transpose()?;
    Ok(crate::survival::EffectDuration {
        amplifier,
        duration,
        hidden,
    })
}

pub fn read_saved_fall_effects(player: &NamedTag) -> Result<crate::survival::FallEffects> {
    let mut effects = crate::survival::FallEffects::default();
    let Some(saved) = player.value.get("active_effects") else {
        return Ok(effects);
    };
    let Tag::List {
        element_type,
        values,
    } = saved
    else {
        bail!("active effects are not a list");
    };
    ensure!(
        *element_type == TagType::Compound || values.is_empty(),
        "active effects are not a compound list"
    );
    for effect in values {
        let Some(name) = effect.get("id").and_then(text) else {
            continue;
        };
        let id = ResourceId::parse(&name)?;
        let slot = match (id.namespace.as_str(), id.path.as_str()) {
            ("minecraft", "night_vision") => &mut effects.night_vision,
            ("minecraft", "jump_boost") => &mut effects.jump_boost,
            ("minecraft", "resistance") => &mut effects.resistance,
            ("minecraft", "slow_falling") => &mut effects.slow_falling,
            ("minecraft", "levitation") => &mut effects.levitation,
            ("minecraft", "speed") => &mut effects.speed,
            ("minecraft", "slowness") => &mut effects.slowness,
            ("minecraft", "dolphins_grace") => &mut effects.dolphins_grace,
            ("minecraft", "haste") => &mut effects.haste,
            ("minecraft", "mining_fatigue") => &mut effects.mining_fatigue,
            ("minecraft", "hunger") => &mut effects.hunger,
            ("minecraft", "poison") => &mut effects.poison,
            ("minecraft", "luck") => &mut effects.luck,
            ("minecraft", "unluck") => &mut effects.unluck,
            _ => continue,
        };
        *slot = Some(effect_duration(effect, 0)?);
    }
    Ok(effects)
}

/// Read the water-breathing effects that are already active when a world is opened.
pub fn read_saved_breathing_effects(
    player: &NamedTag,
) -> Result<crate::survival::BreathingEffects> {
    let mut effects = crate::survival::BreathingEffects::default();
    let Some(saved) = player.value.get("active_effects") else {
        return Ok(effects);
    };
    let Tag::List {
        element_type,
        values,
    } = saved
    else {
        bail!("active effects are not a list");
    };
    ensure!(
        *element_type == TagType::Compound || values.is_empty(),
        "active effects are not a compound list"
    );
    for effect in values {
        let Some(name) = effect.get("id").and_then(text) else {
            continue;
        };
        let id = ResourceId::parse(&name)?;
        let slot = match (id.namespace.as_str(), id.path.as_str()) {
            ("minecraft", "water_breathing") => &mut effects.water_breathing,
            ("minecraft", "conduit_power") => &mut effects.conduit_power,
            ("minecraft", "breath_of_the_nautilus") => &mut effects.breath_of_the_nautilus,
            _ => continue,
        };
        *slot = Some(effect_duration(effect, 0)?);
    }
    Ok(effects)
}

/// Java loads these separately and defaults missing or nonnumeric fields to zero.
pub fn read_saved_experience(player: &NamedTag) -> crate::experience::Experience {
    crate::experience::Experience {
        progress: player
            .value
            .get("XpP")
            .and_then(numeric_float)
            .unwrap_or(0.0),
        level: player
            .value
            .get("XpLevel")
            .and_then(numeric_int)
            .unwrap_or(0),
        total: player
            .value
            .get("XpTotal")
            .and_then(numeric_int)
            .unwrap_or(0),
    }
}

/// Abilities.Packed uses a lenient optional numeric float with a 0.05 default.
pub fn read_saved_flight_speed(player: &NamedTag) -> Result<f32> {
    let speed = player
        .value
        .get("abilities")
        .and_then(|abilities| abilities.get("flySpeed"))
        .and_then(numeric_float)
        .unwrap_or(0.05);
    ensure!(speed.is_finite(), "saved flight speed is not finite");
    Ok(speed)
}

pub fn read_saved_walking_speed(player: &NamedTag) -> Result<f32> {
    let speed = player
        .value
        .get("abilities")
        .and_then(|abilities| abilities.get("walkSpeed"))
        .and_then(numeric_float)
        .unwrap_or(0.1);
    ensure!(speed.is_finite(), "saved walking speed is not finite");
    Ok(speed)
}

pub fn read_saved_flying(player: &NamedTag) -> bool {
    player
        .value
        .get("abilities")
        .and_then(|abilities| abilities.get("flying"))
        .and_then(numeric_double)
        .is_some_and(|value| value != 0.0)
}

/// Read the 26.3 health and food fields without writing the player document.
pub fn read_saved_vitals(
    player: &NamedTag,
    hardcore: bool,
) -> Result<crate::survival::PlayerVitals> {
    let max_health = saved_max_health(player)?;
    let float = |field: &str, default| {
        player
            .value
            .get(field)
            .and_then(numeric_float)
            .unwrap_or(default)
    };
    let integer = |field: &str, default| {
        player
            .value
            .get(field)
            .and_then(numeric_int)
            .unwrap_or(default)
    };
    let health = float("Health", max_health);
    let absorption = float("AbsorptionAmount", 0.0);
    let saturation = float("foodSaturationLevel", 5.0);
    let exhaustion = float("foodExhaustionLevel", 0.0);
    ensure!(
        health.is_finite()
            && absorption.is_finite()
            && saturation.is_finite()
            && exhaustion.is_finite(),
        "saved health or food value is not finite"
    );
    Ok(crate::survival::PlayerVitals {
        health: health.clamp(0.0, max_health),
        max_health,
        absorption: absorption.max(0.0),
        air: integer("Air", 300),
        food: integer("foodLevel", 20),
        saturation,
        exhaustion,
        food_tick_timer: integer("foodTickTimer", 0),
        hardcore,
    })
}

pub(crate) fn java_floor(value: f64) -> i32 {
    // Mth.floor first casts to int, then subtracts one for a negative fraction.
    let integer = value as i32;
    if value < f64::from(integer) {
        integer.wrapping_sub(1)
    } else {
        integer
    }
}

pub(crate) fn numeric_byte(tag: &Tag) -> Option<i8> {
    Some(match tag {
        Tag::Byte(value) => *value,
        Tag::Short(value) => *value as i8,
        Tag::Int(value) => *value as i8,
        Tag::Long(value) => *value as i8,
        Tag::Float(value) => java_floor(f64::from(*value)) as i8,
        Tag::Double(value) => java_floor(*value) as i8,
        _ => return None,
    })
}

fn numeric_vector(tag: &Tag) -> Option<DVec3> {
    let values = match tag {
        Tag::List { values, .. } => {
            if values.iter().any(|v| numeric_double(v).is_none()) {
                return None;
            }
            [
                numeric_double(values.first()?)?,
                numeric_double(values.get(1)?)?,
                numeric_double(values.get(2)?)?,
            ]
        }
        Tag::ByteArray(values) => [
            f64::from(*values.first()? as i8),
            f64::from(*values.get(1)? as i8),
            f64::from(*values.get(2)? as i8),
        ],
        Tag::IntArray(values) => [
            f64::from(*values.first()?),
            f64::from(*values.get(1)?),
            f64::from(*values.get(2)?),
        ],
        Tag::LongArray(values) => [
            *values.first()? as f64,
            *values.get(1)? as f64,
            *values.get(2)? as f64,
        ],
        _ => return None,
    };
    // The Java Vec3 codec returns the first three values as a partial result
    // when the input is longer; a shorter or invalid vector defaults to zero.
    Some(DVec3::new(values[0], values[1], values[2]))
}

/// Read a saved entity position with the 26.3 coordinate limits. An absent
/// position stays absent so the client can ask for another area instead.
pub fn read_saved_position(player: &NamedTag) -> Result<Option<DVec3>> {
    let Some(tag) = player.value.get("Pos") else {
        return Ok(None);
    };
    let position = numeric_vector(tag).context("saved player Pos must contain three numbers")?;
    let position = DVec3::new(
        position.x.clamp(-30_000_512.0, 30_000_512.0),
        position.y.clamp(-20_000_000.0, 20_000_000.0),
        position.z.clamp(-30_000_512.0, 30_000_512.0),
    );
    ensure!(position.is_finite(), "saved player position is not finite");
    Ok(Some(position))
}

fn motion_vector(tag: &Tag) -> Option<DVec3> {
    let values = numeric_vector(tag)?;
    let component = |value: f64| {
        if !value.is_finite() || value.abs() > 10.0 {
            0.0
        } else {
            value
        }
    };
    Some(DVec3::new(
        component(values.x),
        component(values.y),
        component(values.z),
    ))
}

/// Read the motion fields with the defaults and per-axis velocity limits used
/// by 26.3's `Entity.load`. Non-finite fall distance cannot be simulated safely.
pub fn read_saved_motion(player: &NamedTag) -> Result<SavedPlayerMotion> {
    let velocity = player
        .value
        .get("Motion")
        .and_then(motion_vector)
        .unwrap_or(DVec3::ZERO);
    let on_ground = player
        .value
        .get("OnGround")
        .and_then(numeric_byte)
        .is_some_and(|value| value != 0);
    let fall_distance = player
        .value
        .get("fall_distance")
        .and_then(numeric_double)
        .unwrap_or(0.0);
    ensure!(
        fall_distance.is_finite(),
        "saved fall distance is not finite"
    );
    Ok(SavedPlayerMotion {
        velocity,
        on_ground,
        fall_distance,
    })
}

/// ServerPlayer falls back to the world's default if this field is absent.
pub fn saved_game_mode(player: &NamedTag) -> Option<GameMode> {
    player
        .value
        .get("playerGameType")
        .and_then(Tag::as_java_int)
        .map(GameMode::from_legacy_id)
}

pub fn read_saved_player(player: &NamedTag) -> Result<PlayerInventory> {
    let mut inventory = PlayerInventory::default();
    if let Some(book) = read_recipe_book(player) {
        inventory.set_recipe_book(book);
    }
    let selected = match player.value.get("SelectedItemSlot") {
        None => 0,
        Some(Tag::Int(value)) if (0..9).contains(value) => *value as u8,
        _ => bail!("saved SelectedItemSlot must be between 0 and 8"),
    };
    inventory.select(selected);
    match player.value.get("Inventory") {
        None => {}
        Some(Tag::List {
            element_type,
            values,
        }) if *element_type == TagType::Compound || values.is_empty() => {
            for entry in values {
                let slot = match entry.get("Slot") {
                    None => 0,
                    Some(Tag::Byte(value)) => *value as u8,
                    _ => bail!("saved inventory Slot must be an unsigned byte"),
                };
                // 26.3 stores the 36 non-equipment slots here. Invalid slot indices
                // are ignored by the vanilla Inventory.load implementation.
                if slot < 36 {
                    inventory.set_slot(PlayerSlot::Storage(slot), Some(read_stack(entry)?));
                }
            }
        }
        Some(_) => bail!("saved Inventory must be a list of compounds"),
    }
    match player.value.get("equipment") {
        None => {}
        Some(equipment @ Tag::Compound(_)) => {
            for (name, slot) in [
                ("head", PlayerSlot::Armor(0)),
                ("chest", PlayerSlot::Armor(1)),
                ("legs", PlayerSlot::Armor(2)),
                ("feet", PlayerSlot::Armor(3)),
                ("offhand", PlayerSlot::Offhand),
            ] {
                if let Some(item) = equipment.get(name) {
                    inventory.set_slot(
                        slot,
                        Some(read_stack(item).with_context(|| format!("invalid {name} item"))?),
                    );
                }
            }
        }
        Some(_) => bail!("saved equipment must be a compound"),
    }
    Ok(inventory)
}

/// ServerPlayer ignores an absent or undecodable recipeBook codec value.
fn read_recipe_book(player: &NamedTag) -> Option<RecipeBookState> {
    let book = player.value.get("recipeBook")?;
    if !matches!(book, Tag::Compound(_)) {
        return None;
    }
    let read_list = |name: &str| {
        let Tag::List {
            element_type,
            values,
        } = book.get(name)?
        else {
            return None;
        };
        if *element_type != TagType::String && !values.is_empty() {
            return None;
        }
        values
            .iter()
            .map(|value| {
                let Tag::String(value) = value else {
                    return None;
                };
                ResourceId::parse(&String::from_utf16(&value.0).ok()?).ok()
            })
            .collect()
    };
    let read_flag = |name: &str| match book.get(name) {
        None => Some(false),
        Some(value) => Some(value.as_java_int()? != 0),
    };
    Some(RecipeBookState {
        known: read_list("recipes")?,
        highlighted: read_list("toBeDisplayed")?,
        open: read_flag("isGuiOpen")?,
        filtering: read_flag("isFilteringCraftable")?,
    })
}

pub(crate) fn read_stack(value: &Tag) -> Result<ItemStack> {
    ensure!(
        matches!(value, Tag::Compound(_)),
        "item stack must be a compound"
    );
    let id = match value.get("id") {
        Some(Tag::String(value)) => {
            String::from_utf16(&value.0).context("item id contains invalid UTF-16")?
        }
        _ => bail!("item stack has no id"),
    };
    let id = ResourceId::parse(&id).context("invalid item id")?;
    let id = format!("{}:{}", id.namespace, id.path);
    let count = match value.get("count") {
        None => 1,
        Some(Tag::Int(value)) if (1..=99).contains(value) => *value as u32,
        _ => bail!("item count must be between 1 and 99"),
    };
    let components = match value.get("components") {
        None => Tag::Compound(vec![]),
        Some(value @ Tag::Compound(_)) => value.clone(),
        _ => bail!("item components must be a compound"),
    };
    let max = crate::equippable::saved_inventory_max(&id, &components);
    if let Some(max) = max {
        ItemStack::new(id, count, max, components)
    } else {
        ItemStack::unresolved(id, count, components)
    }
    .context("invalid saved item stack")
}

/// An in-memory player NBT draft. It is not sufficient to save a playable world:
/// changed chunks, entities, ticks, lighting and other player state must also be
/// accounted for before a coordinated write can be enabled.
#[derive(Debug)]
pub struct PlayerInventoryDraft {
    source_nbt: Vec<u8>,
    document: NamedTag,
}

/// An in-memory draft only. A coordinated world save must still account for
/// chunks, entities, ticks, other player fields, and the raw source file.
#[derive(Debug)]
pub struct PlayerStateDraft(PlayerInventoryDraft);

impl PlayerStateDraft {
    pub fn document(&self) -> &NamedTag {
        self.0.document()
    }

    pub(crate) fn set_last_death_location(
        &mut self,
        original: &NamedTag,
        target: &crate::lodestone::Target,
    ) -> Result<()> {
        // Refuse to overwrite a saved value whose extra data cannot be preserved.
        read_last_death_location(original)?;
        set_field(
            &mut self.0.document.value,
            "LastDeathLocation",
            Some(target.to_nbt()),
        )?;
        native_persistence::nbt::write(&self.0.document)
            .context("last-death draft cannot be encoded")?;
        Ok(())
    }

    pub fn matches_source(&self, current: &NamedTag) -> Result<bool> {
        self.0.matches_source(current)
    }

    /// Add this player to a partial transaction after checking the exact draft
    /// source and player identity. Other world and player effects still need
    /// coordinated validation before a gameplay save can be committed.
    pub fn add_to_save_plan(
        &self,
        plan: &mut native_persistence::save::WorldSavePlan,
        uuid: [i32; 4],
        source: &NamedTag,
    ) -> Result<()> {
        let matches_uuid = |document: &NamedTag| -> Result<bool> {
            Ok(
                matches!(unique_field(&document.value, "UUID")?, Some(Tag::IntArray(words)) if words.as_slice() == uuid),
            )
        };
        plan.add_player_data(uuid, source, self.document(), |source, replacement| {
            ensure!(self.matches_source(source)?, "player draft source differs");
            ensure!(
                matches_uuid(source)? && matches_uuid(replacement)?,
                "player UUID does not match save target"
            );
            Ok(())
        })
    }

    pub(crate) fn set_effects(&mut self, effects: Option<Tag>) -> Result<()> {
        set_field(&mut self.0.document.value, "active_effects", effects)?;
        native_persistence::nbt::write(&self.0.document)
            .context("player effects draft cannot be encoded")?;
        Ok(())
    }

    /// Update only the persistent status-effect modifiers on movement speed.
    /// Other attribute and modifier data remain byte-for-byte equivalent as NBT.
    pub(crate) fn set_movement_effect_modifiers(
        &mut self,
        original: &NamedTag,
        initial: &SavedAttribute,
        current: &SavedAttribute,
    ) -> Result<()> {
        let ids = [
            ResourceId::parse("minecraft:effect.speed")?,
            ResourceId::parse("minecraft:effect.slowness")?,
        ];
        let mut unchanged_initial = initial.clone();
        let mut unchanged_current = current.clone();
        for id in &ids {
            unchanged_initial.remove_modifier(id);
            unchanged_current.remove_modifier(id);
        }
        ensure!(
            unchanged_initial == unchanged_current,
            "untracked movement attribute change"
        );
        let modifier_changed = ids
            .iter()
            .any(|id| initial.modifier(id) != current.modifier(id));
        let mut source_attribute = read_saved_movement_speed_attribute(original)?;
        let base_changed = source_attribute.base != initial.base;
        source_attribute.set_base(initial.base);
        ensure!(
            source_attribute == *initial,
            "saved movement attribute does not match initial state"
        );
        if !modifier_changed && !base_changed {
            return Ok(());
        }
        let mut attributes = match unique_field(&original.value, "attributes")? {
            Some(Tag::List {
                element_type,
                values,
            }) => {
                ensure!(
                    *element_type == TagType::Compound || values.is_empty(),
                    "attributes are not a compound list"
                );
                values.clone()
            }
            None => Vec::new(),
            Some(_) => bail!("attributes are not a list"),
        };
        let mut movement_index = None;
        let mut attribute_ids = std::collections::BTreeSet::new();
        for (index, attribute) in attributes.iter().enumerate() {
            let id = unique_field(attribute, "id")?
                .and_then(text)
                .context("attribute has no valid id")?;
            let id = ResourceId::parse(&id)?;
            ensure!(attribute_ids.insert(id.clone()), "duplicate attribute id");
            if id.namespace == "minecraft" && id.path == "movement_speed" {
                movement_index = Some(index);
            }
        }
        if movement_index.is_none() {
            ensure!(
                initial.modifiers.is_empty(),
                "missing original movement attribute"
            );
            attributes.push(Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:movement_speed".into())),
                ("base".into(), Tag::Double(initial.base)),
            ]));
            movement_index = Some(attributes.len() - 1);
        }
        let attribute = &mut attributes[movement_index.unwrap()];
        let base = unique_field(attribute, "base")?
            .and_then(numeric_double)
            .context("movement attribute has no numeric base")?;
        ensure!(base.is_finite(), "movement attribute base is not finite");
        if base_changed {
            set_field(attribute, "base", Some(Tag::Double(initial.base)))?;
        }
        let source_modifiers = unique_field(attribute, "modifiers")?;
        let mut modifiers = match source_modifiers {
            None => Vec::new(),
            Some(Tag::List {
                element_type,
                values,
            }) => {
                ensure!(
                    *element_type == TagType::Compound || values.is_empty(),
                    "movement modifiers are not a compound list"
                );
                values.clone()
            }
            Some(_) => bail!("movement modifiers are not a list"),
        };
        let mut modifier_ids = std::collections::BTreeSet::new();
        for modifier in &modifiers {
            let id = unique_field(modifier, "id")?
                .and_then(text)
                .context("movement modifier has no valid id")?;
            let id = ResourceId::parse(&id)?;
            ensure!(modifier_ids.insert(id), "duplicate movement modifier id");
            let amount = unique_field(modifier, "amount")?
                .and_then(numeric_double)
                .context("movement modifier has no amount")?;
            ensure!(amount.is_finite(), "movement modifier amount is not finite");
            let operation = unique_field(modifier, "operation")?
                .and_then(text)
                .context("movement modifier has no operation")?;
            AttributeOperation::parse(&operation)?;
        }
        for id in &ids {
            if initial.modifier(id) == current.modifier(id) {
                continue;
            }
            let index = modifiers.iter().position(|tag| {
                tag.get("id")
                    .and_then(text)
                    .and_then(|value| ResourceId::parse(&value).ok())
                    .as_ref()
                    == Some(id)
            });
            if let Some(index) = index {
                let Tag::Compound(entries) = &modifiers[index] else {
                    unreachable!()
                };
                ensure!(
                    entries.iter().all(|(name, _)| ["id", "amount", "operation"]
                        .iter()
                        .any(|field| name.matches(field))),
                    "changed movement modifier contains unsupported fields"
                );
                modifiers.remove(index);
            }
            if let Some(modifier) = current.modifier(id) {
                ensure!(
                    modifier.operation == AttributeOperation::AddMultipliedTotal
                        && modifier.amount.is_finite(),
                    "unsupported effect movement modifier"
                );
                modifiers.push(Tag::Compound(vec![
                    (
                        "id".into(),
                        Tag::String(format!("{}:{}", id.namespace, id.path).as_str().into()),
                    ),
                    ("amount".into(), Tag::Double(modifier.amount)),
                    (
                        "operation".into(),
                        Tag::String("add_multiplied_total".into()),
                    ),
                ]));
            }
        }
        if modifier_changed {
            set_field(
                attribute,
                "modifiers",
                Some(Tag::List {
                    element_type: TagType::Compound,
                    values: modifiers,
                }),
            )?;
        }
        set_field(
            &mut self.0.document.value,
            "attributes",
            Some(Tag::List {
                element_type: TagType::Compound,
                values: attributes,
            }),
        )?;
        native_persistence::nbt::write(&self.0.document)
            .context("movement attribute draft cannot be encoded")?;
        Ok(())
    }

    pub(crate) fn set_flying(&mut self, original: &NamedTag, flying: bool) -> Result<()> {
        let source = unique_field(&original.value, "abilities")?;
        if let Some(value @ Tag::Compound(_)) = source {
            unique_field(value, "flying")?;
            if let Some(walking) = unique_field(value, "walkSpeed")? {
                ensure!(
                    numeric_float(walking).is_some_and(f32::is_finite),
                    "walking speed cannot be preserved safely"
                );
            }
        }
        if read_saved_flying(original) == flying {
            return Ok(());
        }
        let mut abilities = match source {
            Some(value @ Tag::Compound(_)) => value.clone(),
            None => Tag::Compound(Vec::new()),
            Some(_) => bail!("saved abilities cannot be preserved safely"),
        };
        set_field(&mut abilities, "flying", Some(Tag::Byte(i8::from(flying))))?;
        set_field(&mut self.0.document.value, "abilities", Some(abilities))?;
        native_persistence::nbt::write(&self.0.document)
            .context("player flight draft cannot be encoded")?;
        Ok(())
    }
}

impl PlayerInventoryDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }

    /// Before any eventual write, compare against the current player document
    /// while holding the world lock. The file writer must also check the raw
    /// bytes and coordinate this with all other world changes.
    pub fn matches_source(&self, current: &NamedTag) -> Result<bool> {
        Ok(native_persistence::nbt::write(current)? == self.source_nbt)
    }
}

fn unique_field<'a>(parent: &'a Tag, field: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(entries) = parent else {
        bail!("{field} parent is not a compound");
    };
    let mut found = entries.iter().filter(|(name, _)| name.matches(field));
    let value = found.next().map(|(_, value)| value);
    ensure!(
        found.next().is_none(),
        "duplicate {field} fields cannot be saved safely"
    );
    Ok(value)
}

fn set_field(parent: &mut Tag, field: &str, replacement: Option<Tag>) -> Result<()> {
    unique_field(parent, field)?;
    let Tag::Compound(entries) = parent else {
        unreachable!()
    };
    if let Some(index) = entries.iter().position(|(name, _)| name.matches(field)) {
        if let Some(value) = replacement {
            entries[index].1 = value;
        } else {
            entries.remove(index);
        }
    } else if let Some(value) = replacement {
        entries.push((field.into(), value));
    }
    Ok(())
}

/// Prepare the saved representation of the effects currently simulated by the
/// player session. This is an in-memory draft, not authorization to save a world.
/// Unknown effects and fields are rejected because their behavior or promotion
/// cannot yet be preserved. All hidden effects age while the visible one runs.
pub fn advance_saved_effects(player: &NamedTag, ticks: u64) -> Result<Option<Tag>> {
    let Some(saved) = unique_field(&player.value, "active_effects")? else {
        return Ok(None);
    };
    let Tag::List {
        element_type,
        values,
    } = saved
    else {
        bail!("active effects are not a list");
    };
    ensure!(
        *element_type == TagType::Compound || values.is_empty(),
        "active effects are not a compound list"
    );

    fn validate(tag: &Tag, root: bool, depth: usize) -> Result<i32> {
        ensure!(depth <= 256, "status effect nesting is too deep");
        let Tag::Compound(entries) = tag else {
            bail!("status effect is not a compound");
        };
        for (name, _) in entries {
            ensure!(
                [
                    "amplifier",
                    "duration",
                    "ambient",
                    "show_particles",
                    "show_icon",
                    "hidden_effect"
                ]
                .iter()
                .any(|field| name.matches(field))
                    || (root && name.matches("id")),
                "unknown status effect data cannot be preserved safely"
            );
        }
        for field in [
            "id",
            "amplifier",
            "duration",
            "ambient",
            "show_particles",
            "show_icon",
            "hidden_effect",
        ] {
            unique_field(tag, field)?;
        }
        if let Some(value) = tag.get("amplifier") {
            ensure!(
                matches!(value, Tag::Byte(_)),
                "invalid status effect amplifier"
            );
        }
        for field in ["ambient", "show_particles", "show_icon"] {
            if let Some(value) = tag.get(field) {
                ensure!(
                    matches!(value, Tag::Byte(0 | 1)),
                    "invalid status effect {field}"
                );
            }
        }
        let duration = match tag.get("duration") {
            None => 0,
            Some(Tag::Int(value)) => *value,
            Some(_) => bail!("invalid status effect duration"),
        };
        ensure!(
            duration >= -1,
            "unsupported negative status effect duration"
        );
        if let Some(hidden) = tag.get("hidden_effect") {
            validate(hidden, false, depth + 1)?;
        }
        Ok(duration)
    }

    fn age(tag: &mut Tag, ticks: u64) -> Result<()> {
        let duration = match tag.get("duration") {
            Some(Tag::Int(value)) => *value,
            None => 0,
            _ => unreachable!(),
        };
        if duration > 0 {
            let remaining = (u64::from(duration as u32)).saturating_sub(ticks) as i32;
            if remaining != duration {
                set_field(tag, "duration", Some(Tag::Int(remaining)))?;
            }
        }
        let Tag::Compound(entries) = tag else {
            unreachable!()
        };
        if let Some((_, hidden)) = entries
            .iter_mut()
            .find(|(name, _)| name.matches("hidden_effect"))
        {
            age(hidden, ticks)?;
        }
        Ok(())
    }

    let mut seen = std::collections::HashSet::new();
    let mut advanced = Vec::with_capacity(values.len());
    for source in values {
        validate(source, true, 0)?;
        let id = unique_field(source, "id")?
            .and_then(text)
            .context("status effect has no valid id")?;
        let resource = ResourceId::parse(&id)?;
        ensure!(
            resource.namespace == "minecraft"
                && matches!(
                    resource.path.as_str(),
                    "jump_boost"
                        | "night_vision"
                        | "resistance"
                        | "slow_falling"
                        | "levitation"
                        | "speed"
                        | "slowness"
                        | "dolphins_grace"
                        | "water_breathing"
                        | "conduit_power"
                        | "breath_of_the_nautilus"
                        | "hunger"
                        | "poison"
                        | "luck"
                        | "unluck"
                ),
            "unsupported status effect {id}"
        );
        ensure!(
            seen.insert(resource.path),
            "duplicate status effects cannot be saved safely"
        );
        let source_id = source.get("id").unwrap().clone();
        let mut effect = source.clone();
        let mut remaining = ticks;
        let mut expired = false;
        while remaining > 0 {
            let duration = validate(&effect, true, 0)?;
            if duration == 0 {
                expired = true;
                break;
            }
            let step = if duration == -1 {
                remaining
            } else {
                remaining.min(duration as u64)
            };
            age(&mut effect, step)?;
            remaining -= step;
            if duration != -1 && step == duration as u64 {
                if let Some(hidden) = effect.get("hidden_effect").cloned() {
                    effect = hidden;
                    set_field(&mut effect, "id", Some(source_id.clone()))?;
                    if validate(&effect, true, 0)? == 0 {
                        expired = true;
                        break;
                    }
                } else {
                    expired = true;
                    break;
                }
            }
        }
        if !expired {
            advanced.push(effect);
        }
    }
    Ok(Some(Tag::List {
        element_type: *element_type,
        values: advanced,
    }))
}

/// Replay known food-applied hunger effects at the point they occurred between
/// base ticks. Unknown effect data still prevents a draft from being written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HungerUpdate {
    pub tick: u64,
    pub duration: i32,
    pub amplifier: u8,
    pub ambient: bool,
    pub visible: bool,
    pub show_icon: bool,
}

impl HungerUpdate {
    #[cfg(test)]
    fn rotten_flesh(tick: u64) -> Self {
        Self {
            tick,
            duration: 600,
            amplifier: 0,
            ambient: false,
            visible: true,
            show_icon: true,
        }
    }
}

pub fn replay_saved_hunger_effects(
    player: &NamedTag,
    ticks: u64,
    hunger_updates: &[HungerUpdate],
) -> Result<Option<Tag>> {
    let updates: Vec<_> = hunger_updates
        .iter()
        .copied()
        .map(|update| StatusUpdate {
            kind: crate::survival::FoodStatusKind::Hunger,
            update,
            remove: false,
            clear_all: false,
        })
        .collect();
    replay_saved_status_effects(player, ticks, &updates)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StatusUpdate {
    pub kind: crate::survival::FoodStatusKind,
    pub update: HungerUpdate,
    pub remove: bool,
    pub clear_all: bool,
}

pub(crate) fn replay_saved_status_effects(
    player: &NamedTag,
    ticks: u64,
    updates: &[StatusUpdate],
) -> Result<Option<Tag>> {
    fn as_player(effects: Option<Tag>) -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(
                effects
                    .map(|value| ("active_effects".into(), value))
                    .into_iter()
                    .collect(),
            ),
        }
    }

    fn new_effect(root: bool, kind: crate::survival::FoodStatusKind, update: HungerUpdate) -> Tag {
        let mut fields = vec![
            ("duration".into(), Tag::Int(update.duration)),
            ("amplifier".into(), Tag::Byte(update.amplifier as i8)),
            ("ambient".into(), Tag::Byte(i8::from(update.ambient))),
            ("show_particles".into(), Tag::Byte(i8::from(update.visible))),
            ("show_icon".into(), Tag::Byte(i8::from(update.show_icon))),
        ];
        if root {
            fields.insert(
                0,
                (
                    "id".into(),
                    Tag::String(
                        match kind {
                            crate::survival::FoodStatusKind::Hunger => "minecraft:hunger",
                            crate::survival::FoodStatusKind::Poison => "minecraft:poison",
                            crate::survival::FoodStatusKind::NightVision => {
                                "minecraft:night_vision"
                            }
                            crate::survival::FoodStatusKind::WaterBreathing => {
                                "minecraft:water_breathing"
                            }
                        }
                        .into(),
                    ),
                ),
            );
        }
        Tag::Compound(fields)
    }

    fn merge(
        effect: &mut Tag,
        depth: usize,
        kind: crate::survival::FoodStatusKind,
        update: HungerUpdate,
    ) -> Result<()> {
        ensure!(depth <= 256, "status effect nesting is too deep");
        let duration = match effect.get("duration") {
            Some(Tag::Int(value)) => *value,
            None => 0,
            _ => bail!("invalid hunger duration"),
        };
        let amplifier = match effect.get("amplifier") {
            Some(Tag::Byte(value)) => *value as u8,
            None => 0,
            _ => bail!("invalid hunger amplifier"),
        };
        let shorter = |left: i32, right: i32| left != -1 && (right == -1 || left < right);
        let mut changed = false;
        if update.amplifier > amplifier {
            if shorter(update.duration, duration) {
                ensure!(depth < 256, "status effect nesting is too deep");
                let mut previous = effect.clone();
                set_field(&mut previous, "id", None)?;
                set_field(effect, "hidden_effect", Some(previous))?;
            }
            set_field(effect, "duration", Some(Tag::Int(update.duration)))?;
            set_field(effect, "amplifier", Some(Tag::Byte(update.amplifier as i8)))?;
            changed = true;
        } else if shorter(duration, update.duration) {
            if amplifier == update.amplifier {
                set_field(effect, "duration", Some(Tag::Int(update.duration)))?;
                changed = true;
            } else if let Some(mut hidden) = effect.get("hidden_effect").cloned() {
                merge(&mut hidden, depth + 1, kind, update)?;
                set_field(effect, "hidden_effect", Some(hidden))?;
            } else {
                ensure!(depth < 256, "status effect nesting is too deep");
                set_field(
                    effect,
                    "hidden_effect",
                    Some(new_effect(false, kind, update)),
                )?;
            }
        }
        let was_ambient = matches!(effect.get("ambient"), Some(Tag::Byte(1)));
        if (!update.ambient && was_ambient) || changed {
            set_field(effect, "ambient", Some(Tag::Byte(i8::from(update.ambient))))?;
        }
        set_field(
            effect,
            "show_particles",
            Some(Tag::Byte(i8::from(update.visible))),
        )?;
        set_field(
            effect,
            "show_icon",
            Some(Tag::Byte(i8::from(update.show_icon))),
        )?;
        Ok(())
    }

    let mut effects = advance_saved_effects(player, 0)?;
    let mut elapsed = 0;
    for &StatusUpdate {
        kind,
        update,
        remove,
        clear_all,
    } in updates
    {
        let event_tick = update.tick;
        ensure!(
            event_tick >= elapsed && event_tick <= ticks,
            "invalid hunger update order"
        );
        ensure!(update.duration >= -1, "unsupported hunger duration");
        effects = advance_saved_effects(&as_player(effects), event_tick - elapsed)?;
        let mut list = effects.unwrap_or(Tag::List {
            element_type: TagType::Compound,
            values: Vec::new(),
        });
        let Tag::List {
            element_type,
            values,
        } = &mut list
        else {
            unreachable!()
        };
        *element_type = TagType::Compound;
        if clear_all {
            ensure!(remove, "invalid clear-all status update");
            values.clear();
            effects = Some(list);
            elapsed = event_tick;
            continue;
        }
        if remove {
            values.retain(|effect| {
                !effect
                    .get("id")
                    .and_then(text)
                    .and_then(|id| ResourceId::parse(&id).ok())
                    .is_some_and(|id| {
                        id.namespace == "minecraft"
                            && id.path
                                == match kind {
                                    crate::survival::FoodStatusKind::Hunger => "hunger",
                                    crate::survival::FoodStatusKind::Poison => "poison",
                                    crate::survival::FoodStatusKind::NightVision => "night_vision",
                                    crate::survival::FoodStatusKind::WaterBreathing => {
                                        "water_breathing"
                                    }
                                }
                    })
            });
            effects = Some(list);
            elapsed = event_tick;
            continue;
        }
        if let Some(effect) = values.iter_mut().find(|effect| {
            effect
                .get("id")
                .and_then(text)
                .and_then(|id| ResourceId::parse(&id).ok())
                .is_some_and(|id| {
                    id.namespace == "minecraft"
                        && id.path
                            == match kind {
                                crate::survival::FoodStatusKind::Hunger => "hunger",
                                crate::survival::FoodStatusKind::Poison => "poison",
                                crate::survival::FoodStatusKind::NightVision => "night_vision",
                                crate::survival::FoodStatusKind::WaterBreathing => {
                                    "water_breathing"
                                }
                            }
                })
        }) {
            merge(effect, 0, kind, update)?;
        } else {
            values.push(new_effect(true, kind, update));
        }
        effects = Some(list);
        elapsed = event_tick;
    }
    advance_saved_effects(&as_player(effects), ticks - elapsed)
}

fn validate_max_health_for_draft(player: &NamedTag) -> Result<f32> {
    let Some(attributes) = unique_field(&player.value, "attributes")? else {
        return Ok(20.0);
    };
    let Tag::List {
        element_type,
        values,
    } = attributes
    else {
        bail!("attributes are not a list");
    };
    ensure!(
        *element_type == TagType::Compound || values.is_empty(),
        "attributes are not a list of compounds"
    );
    let mut attribute_ids = std::collections::BTreeSet::new();
    for attribute in values {
        let id = unique_field(attribute, "id")?
            .and_then(text)
            .context("attribute has no valid id")?;
        let id = ResourceId::parse(&id).context("invalid attribute id")?;
        ensure!(attribute_ids.insert(id.clone()), "duplicate attribute id");
        if id.namespace != "minecraft" || id.path != "max_health" {
            continue;
        }
        let base = unique_field(attribute, "base")?
            .and_then(numeric_double)
            .context("maximum health attribute has no numeric base")?;
        ensure!(
            base.is_finite(),
            "maximum health attribute base is not finite"
        );
        let Some(modifiers) = unique_field(attribute, "modifiers")? else {
            continue;
        };
        let Tag::List {
            element_type,
            values,
        } = modifiers
        else {
            bail!("maximum health modifiers are not a list");
        };
        ensure!(
            *element_type == TagType::Compound || values.is_empty(),
            "maximum health modifiers are not compounds"
        );
        let mut modifier_ids = std::collections::BTreeSet::new();
        for modifier in values {
            let id = unique_field(modifier, "id")?
                .and_then(text)
                .context("maximum health modifier has no valid id")?;
            ensure!(
                modifier_ids.insert(ResourceId::parse(&id)?),
                "duplicate maximum health modifier id"
            );
            let amount = unique_field(modifier, "amount")?
                .and_then(numeric_double)
                .context("maximum health modifier has no numeric amount")?;
            ensure!(amount.is_finite(), "maximum health modifier is not finite");
            let operation = unique_field(modifier, "operation")?
                .and_then(text)
                .context("maximum health modifier has no valid operation")?;
            ensure!(
                matches!(
                    operation.strip_prefix("minecraft:").unwrap_or(&operation),
                    "add_value" | "add_multiplied_base" | "add_multiplied_total"
                ),
                "unsupported maximum health modifier operation"
            );
        }
    }
    saved_max_health(player)
}

pub(crate) fn validate_original_stack(stack: &Tag, has_slot: bool) -> Result<()> {
    let Tag::Compound(entries) = stack else {
        bail!("saved item is not a compound")
    };
    for (name, _) in entries {
        ensure!(
            ["id", "count", "components"]
                .iter()
                .any(|field| name.matches(field))
                || (has_slot && name.matches("Slot")),
            "item has an unrecognized field that cannot be preserved when moved"
        );
    }
    for field in ["id", "count", "components"] {
        unique_field(stack, field)?;
    }
    if has_slot {
        ensure!(
            matches!(unique_field(stack, "Slot")?, Some(Tag::Byte(_))),
            "inventory entry has no valid Slot"
        );
    }
    read_stack(stack)?;
    Ok(())
}

fn validate_original_inventory(player: &NamedTag) -> Result<()> {
    for field in [
        "DataVersion",
        "Inventory",
        "SelectedItemSlot",
        "equipment",
        "recipeBook",
    ] {
        unique_field(&player.value, field)?;
    }
    ensure!(
        matches!(unique_field(&player.value, "DataVersion")?, Some(Tag::Int(version))
        if *version == native_persistence::world::TARGET_DATA_VERSION),
        "player data must use Minecraft 26.3 DataVersion 5023"
    );
    if let Some(inventory) = unique_field(&player.value, "Inventory")? {
        let Tag::List {
            element_type,
            values,
        } = inventory
        else {
            bail!("Inventory is not a list")
        };
        ensure!(
            *element_type == TagType::Compound || values.is_empty(),
            "Inventory is not a list of compounds"
        );
        let mut slots = [false; 36];
        for stack in values {
            validate_original_stack(stack, true)?;
            let Some(Tag::Byte(slot)) = stack.get("Slot") else {
                unreachable!()
            };
            let slot = *slot as u8 as usize;
            ensure!(
                slot < slots.len(),
                "inventory contains an unsupported slot {slot}"
            );
            ensure!(!slots[slot], "inventory contains duplicate slot {slot}");
            slots[slot] = true;
        }
    }
    if let Some(equipment) = unique_field(&player.value, "equipment")? {
        ensure!(
            matches!(equipment, Tag::Compound(_)),
            "equipment is not a compound"
        );
        for name in ["head", "chest", "legs", "feet", "offhand"] {
            if let Some(stack) = unique_field(equipment, name)? {
                validate_original_stack(stack, false)
                    .with_context(|| format!("cannot preserve {name} equipment"))?;
            }
        }
    }
    Ok(())
}

pub(crate) fn saved_slots(inventory: &PlayerInventory) -> impl Iterator<Item = &ItemStack> {
    (0..36)
        .map(PlayerSlot::Storage)
        .chain((0..4).map(PlayerSlot::Armor))
        .chain([PlayerSlot::Offhand])
        .filter_map(|slot| inventory.slot(slot))
}

fn validate_runtime_inventory(
    original: &PlayerInventory,
    inventory: &PlayerInventory,
) -> Result<()> {
    ensure!(
        inventory.ready_for_player_save(),
        "open containers or carried items cannot be saved yet"
    );
    let mut unresolved: Vec<_> = saved_slots(original)
        .filter(|item| item.max_count().is_none())
        .collect();
    for item in saved_slots(inventory) {
        ensure!(
            (1..=99).contains(&item.count()),
            "item count is outside the saved codec range"
        );
        ResourceId::parse(item.id()).context("invalid saved item id")?;
        ensure!(
            matches!(item.components(), Tag::Compound(_)),
            "item components are not a compound"
        );
        if item.max_count().is_none() {
            let index = unresolved
                .iter()
                .position(|source| *source == item)
                .context("an unresolved item was created or modified")?;
            unresolved.swap_remove(index);
        } else {
            ensure!(
                crate::equippable::saved_inventory_max(item.id(), item.components())
                    == item.max_count(),
                "item stack limit cannot be reconstructed from its saved components"
            );
        }
    }
    ensure!(unresolved.is_empty(), "an unresolved item would be lost");
    Ok(())
}

pub(crate) fn encode_stack(item: &ItemStack, slot: Option<u8>) -> Tag {
    let mut fields = Vec::new();
    if let Some(slot) = slot {
        fields.push(("Slot".into(), Tag::Byte(slot as i8)));
    }
    fields.push(("id".into(), Tag::String(item.id().into())));
    fields.push(("count".into(), Tag::Int(item.count() as i32)));
    if !matches!(item.components(), Tag::Compound(values) if values.is_empty()) {
        fields.push(("components".into(), item.components().clone()));
    }
    Tag::Compound(fields)
}

fn validate_recipe_list(book: &Tag, field: &str) -> Result<()> {
    let Some(Tag::List {
        element_type,
        values,
    }) = unique_field(book, field)?
    else {
        bail!("recipe book {field} is not a list");
    };
    ensure!(
        *element_type == TagType::String || values.is_empty(),
        "recipe book {field} is not a string list"
    );
    let mut seen = std::collections::BTreeSet::new();
    for value in values {
        let Tag::String(value) = value else {
            bail!("recipe book {field} contains a non-string")
        };
        let value = String::from_utf16(&value.0).context("invalid recipe identifier encoding")?;
        ensure!(
            seen.insert(ResourceId::parse(&value)?),
            "recipe book {field} contains duplicate entries"
        );
    }
    Ok(())
}

fn write_recipe_book(
    original: &NamedTag,
    document: &mut NamedTag,
    inventory: &PlayerInventory,
) -> Result<()> {
    let source = unique_field(&original.value, "recipeBook")?;
    let decoded = read_recipe_book(original);
    if decoded.as_ref() == Some(inventory.recipe_book())
        || (decoded.is_none() && inventory.recipe_book() == &RecipeBookState::default())
    {
        return Ok(());
    }
    let mut book = if let Some(book) = source {
        ensure!(
            decoded.is_some(),
            "unrecognized recipe book data cannot be modified safely"
        );
        for field in [
            "recipes",
            "toBeDisplayed",
            "isGuiOpen",
            "isFilteringCraftable",
        ] {
            unique_field(book, field)?;
        }
        validate_recipe_list(book, "recipes")?;
        validate_recipe_list(book, "toBeDisplayed")?;
        book.clone()
    } else {
        Tag::Compound(Vec::new())
    };
    let state = inventory.recipe_book();
    let list = |ids: &std::collections::BTreeSet<ResourceId>| Tag::List {
        element_type: TagType::String,
        values: ids
            .iter()
            .map(|id| Tag::String(format!("{}:{}", id.namespace, id.path).as_str().into()))
            .collect(),
    };
    set_field(&mut book, "recipes", Some(list(&state.known)))?;
    set_field(&mut book, "toBeDisplayed", Some(list(&state.highlighted)))?;
    set_field(
        &mut book,
        "isGuiOpen",
        Some(Tag::Byte(i8::from(state.open))),
    )?;
    set_field(
        &mut book,
        "isFilteringCraftable",
        Some(Tag::Byte(i8::from(state.filtering))),
    )?;
    set_field(&mut document.value, "recipeBook", Some(book))
}

/// Prepare an inventory update while retaining unrelated and unknown player data.
/// Unsupported or ambiguous data is rejected; no file is read or written.
pub fn draft_player_inventory(
    original: &NamedTag,
    inventory: &PlayerInventory,
) -> Result<PlayerInventoryDraft> {
    validate_original_inventory(original)?;
    let source = read_saved_player(original)?;
    validate_runtime_inventory(&source, inventory)?;
    let mut document = original.clone();
    let slots: Vec<_> = (0..36)
        .filter_map(|slot| {
            inventory
                .slot(PlayerSlot::Storage(slot))
                .map(|item| encode_stack(item, Some(slot)))
        })
        .collect();
    if !slots.is_empty() || original.value.get("Inventory").is_some() {
        set_field(
            &mut document.value,
            "Inventory",
            Some(Tag::List {
                element_type: TagType::Compound,
                values: slots,
            }),
        )?;
    }
    if inventory.selected() != 0 || original.value.get("SelectedItemSlot").is_some() {
        set_field(
            &mut document.value,
            "SelectedItemSlot",
            Some(Tag::Int(i32::from(inventory.selected()))),
        )?;
    }
    let equipment_slots = [
        ("head", PlayerSlot::Armor(0)),
        ("chest", PlayerSlot::Armor(1)),
        ("legs", PlayerSlot::Armor(2)),
        ("feet", PlayerSlot::Armor(3)),
        ("offhand", PlayerSlot::Offhand),
    ];
    let existing = original.value.get("equipment");
    if existing.is_some()
        || equipment_slots
            .iter()
            .any(|(_, slot)| inventory.slot(*slot).is_some())
    {
        let mut equipment = existing
            .cloned()
            .unwrap_or_else(|| Tag::Compound(Vec::new()));
        for (name, slot) in equipment_slots {
            set_field(
                &mut equipment,
                name,
                inventory.slot(slot).map(|item| encode_stack(item, None)),
            )?;
        }
        set_field(&mut document.value, "equipment", Some(equipment))?;
    }
    write_recipe_book(original, &mut document, inventory)?;
    native_persistence::nbt::write(&document).context("draft player NBT cannot be encoded")?;
    let source_nbt = native_persistence::nbt::write(original)
        .context("original player NBT cannot be encoded")?;
    Ok(PlayerInventoryDraft {
        source_nbt,
        document,
    })
}

/// Build the currently simulated player fields and inventory into one draft.
/// This does not authorize writing: unrelated time-dependent state and every
/// other changed world document still need a coordinated preservation check.
pub fn draft_player_state(
    original: &NamedTag,
    state: &PlayerSaveState,
    inventory: &PlayerInventory,
) -> Result<PlayerStateDraft> {
    for field in [
        "Pos",
        "Motion",
        "Rotation",
        "fall_distance",
        "OnGround",
        "Dimension",
        "NoGravity",
        "playerGameType",
        "Air",
        "Health",
        "AbsorptionAmount",
        "foodLevel",
        "foodTickTimer",
        "foodSaturationLevel",
        "foodExhaustionLevel",
        "XpP",
        "XpLevel",
        "XpTotal",
        "Score",
    ] {
        unique_field(&original.value, field)?;
    }
    if let Some(tag) = unique_field(&original.value, "NoGravity")? {
        ensure!(
            numeric_byte(tag).is_some(),
            "NoGravity cannot be preserved safely"
        );
    }
    let validate_vector = |field: &str, element_type, length| -> Result<()> {
        if let Some(tag) = unique_field(&original.value, field)? {
            ensure!(
                matches!(tag, Tag::List { element_type: actual, values }
                if *actual == element_type && values.len() == length
                && values.iter().all(|value| match value {
                    Tag::Float(v) => v.is_finite(),
                    Tag::Double(v) => v.is_finite(),
                    _ => false,
                })),
                "{field} cannot be preserved safely"
            );
        }
        Ok(())
    };
    validate_vector("Pos", TagType::Double, 3)?;
    validate_vector("Motion", TagType::Double, 3)?;
    validate_vector("Rotation", TagType::Float, 2)?;
    if let Some(position) = unique_field(&original.value, "Pos")?.and_then(numeric_vector) {
        ensure!(
            (-30_000_512.0..=30_000_512.0).contains(&position.x)
                && (-20_000_000.0..=20_000_000.0).contains(&position.y)
                && (-30_000_512.0..=30_000_512.0).contains(&position.z),
            "original position would be clamped on reload"
        );
    }
    if let Some(velocity) = unique_field(&original.value, "Motion")?.and_then(numeric_vector) {
        ensure!(
            velocity.abs().max_element() <= 10.0,
            "original velocity would be discarded on reload"
        );
    }
    for field in [
        "Health",
        "AbsorptionAmount",
        "foodSaturationLevel",
        "foodExhaustionLevel",
    ] {
        if let Some(tag) = unique_field(&original.value, field)? {
            ensure!(
                matches!(tag, Tag::Float(v) if v.is_finite()),
                "{field} cannot be preserved safely"
            );
        }
    }
    let source_max_health = validate_max_health_for_draft(original)?;
    if let Some(Tag::Float(health)) = unique_field(&original.value, "Health")? {
        ensure!(
            (0.0..=source_max_health).contains(health),
            "original health would be clamped on reload"
        );
    }
    if let Some(Tag::Float(absorption)) = unique_field(&original.value, "AbsorptionAmount")? {
        ensure!(
            *absorption >= 0.0,
            "original absorption would be clamped on reload"
        );
    }
    for field in ["foodLevel", "foodTickTimer"] {
        if let Some(tag) = unique_field(&original.value, field)? {
            ensure!(
                matches!(tag, Tag::Int(_)),
                "{field} cannot be preserved safely"
            );
        }
    }
    if let Some(tag) = unique_field(&original.value, "XpP")? {
        ensure!(
            matches!(tag, Tag::Float(value) if value.is_finite()),
            "XpP cannot be preserved safely"
        );
    }
    for field in ["XpLevel", "XpTotal", "Score"] {
        if let Some(tag) = unique_field(&original.value, field)? {
            ensure!(
                matches!(tag, Tag::Int(_)),
                "{field} cannot be preserved safely"
            );
        }
    }
    ensure!(
        state.experience.progress.is_finite(),
        "experience progress cannot be saved safely"
    );
    if let Some(tag) = unique_field(&original.value, "fall_distance")? {
        ensure!(
            matches!(tag, Tag::Double(v) if v.is_finite()),
            "fall_distance cannot be preserved safely"
        );
    }
    if let Some(tag) = unique_field(&original.value, "OnGround")? {
        ensure!(
            matches!(tag, Tag::Byte(0 | 1)),
            "OnGround cannot be preserved safely"
        );
    }
    if let Some(tag) = unique_field(&original.value, "Air")? {
        ensure!(
            matches!(tag, Tag::Short(_)),
            "Air cannot be preserved safely"
        );
    }
    if let Some(tag) = unique_field(&original.value, "playerGameType")? {
        ensure!(
            matches!(tag, Tag::Int(id) if (0..=3).contains(id) && GameMode::from_legacy_id(*id) == state.game_mode),
            "player game mode changed or cannot be preserved safely"
        );
    }
    let dimension = native_persistence::world::DimensionId::parse(&state.dimension)?;
    if let Some(tag) = unique_field(&original.value, "Dimension")? {
        let saved = text(tag).context("Dimension is not a valid string")?;
        ensure!(
            native_persistence::world::DimensionId::parse(&saved)? == dimension,
            "player dimension changed and cannot yet be saved safely"
        );
    }
    let position = state.position;
    ensure!(
        position.is_finite()
            && (-30_000_512.0..=30_000_512.0).contains(&position.x)
            && (-20_000_000.0..=20_000_000.0).contains(&position.y)
            && (-30_000_512.0..=30_000_512.0).contains(&position.z),
        "player position is not safely reloadable"
    );
    ensure!(
        state.motion.velocity.is_finite()
            && state.motion.velocity.abs().max_element() <= 10.0
            && state.motion.fall_distance.is_finite()
            && state.yaw.is_finite()
            && state.pitch.is_finite(),
        "player motion or rotation is not safely reloadable"
    );
    let vitals = state.vitals;
    ensure!(
        vitals.health.is_finite()
            && vitals.max_health.is_finite()
            && vitals.absorption.is_finite()
            && vitals.saturation.is_finite()
            && vitals.exhaustion.is_finite()
            && vitals.health >= 0.0
            && vitals.health <= vitals.max_health
            && vitals.absorption >= 0.0,
        "player health or food is not safely reloadable"
    );
    ensure!(
        i16::try_from(vitals.air).is_ok(),
        "player air is not safely reloadable"
    );
    ensure!(
        vitals.max_health == source_max_health,
        "maximum health changed without a supported attribute update"
    );

    let mut draft = draft_player_inventory(original, inventory)?;
    let document = &mut draft.document.value;
    let vector = |value: DVec3| Tag::List {
        element_type: TagType::Double,
        values: value.to_array().into_iter().map(Tag::Double).collect(),
    };
    set_field(document, "Pos", Some(vector(position)))?;
    set_field(document, "Motion", Some(vector(state.motion.velocity)))?;
    set_field(
        document,
        "Rotation",
        Some(Tag::List {
            element_type: TagType::Float,
            values: vec![Tag::Float(state.yaw), Tag::Float(state.pitch)],
        }),
    )?;
    set_field(
        document,
        "fall_distance",
        Some(Tag::Double(state.motion.fall_distance)),
    )?;
    set_field(
        document,
        "OnGround",
        Some(Tag::Byte(i8::from(state.motion.on_ground))),
    )?;
    let dimension = format!("{}:{}", dimension.namespace, dimension.path);
    set_field(
        document,
        "Dimension",
        Some(Tag::String(dimension.as_str().into())),
    )?;
    for (name, value) in [
        ("Health", Tag::Float(vitals.health)),
        ("AbsorptionAmount", Tag::Float(vitals.absorption)),
        ("Air", Tag::Short(vitals.air as i16)),
        ("foodLevel", Tag::Int(vitals.food)),
        ("foodTickTimer", Tag::Int(vitals.food_tick_timer)),
        ("foodSaturationLevel", Tag::Float(vitals.saturation)),
        ("foodExhaustionLevel", Tag::Float(vitals.exhaustion)),
        ("XpP", Tag::Float(state.experience.progress)),
        ("XpLevel", Tag::Int(state.experience.level)),
        ("XpTotal", Tag::Int(state.experience.total)),
        ("Score", Tag::Int(state.score)),
    ] {
        set_field(document, name, Some(value))?;
    }
    native_persistence::nbt::write(&draft.document)
        .context("player state draft cannot be encoded")?;
    Ok(PlayerStateDraft(draft))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_luck_rejects_ambiguous_attributes_and_unknown_effects() {
        let attribute = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:luck".into())),
            ("base".into(), Tag::Double(4.5)),
        ]);
        let make = |attributes: Vec<Tag>, effects: Vec<Tag>| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                (
                    "attributes".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: attributes,
                    },
                ),
                (
                    "active_effects".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: effects,
                    },
                ),
            ]),
        };
        assert_eq!(
            read_saved_luck_attribute(&make(vec![attribute.clone()], vec![]))
                .unwrap()
                .value(-1024.0, 1024.0)
                .unwrap(),
            4.5
        );
        assert!(
            read_saved_luck_attribute(&make(vec![attribute.clone(), attribute], vec![])).is_err()
        );
        let effect = |id: &str| {
            Tag::Compound(vec![
                ("id".into(), Tag::String(id.into())),
                ("amplifier".into(), Tag::Byte(0)),
                ("duration".into(), Tag::Int(100)),
            ])
        };
        assert!(read_saved_luck_attribute(&make(vec![], vec![effect("example:unknown")])).is_err());
        assert!(
            read_saved_luck_attribute(&make(
                vec![],
                vec![effect("minecraft:luck"), effect("minecraft:luck")]
            ))
            .is_err()
        );
        let good = make(
            vec![],
            vec![effect("minecraft:luck"), effect("minecraft:unluck")],
        );
        assert!(read_saved_luck_attribute(&good).is_ok());
        let effects = read_saved_fall_effects(&good).unwrap();
        assert!(effects.luck.is_some() && effects.unluck.is_some());
        let advanced = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "active_effects".into(),
                advance_saved_effects(&good, 1).unwrap().unwrap(),
            )]),
        };
        let mut expected = effects;
        expected.tick();
        assert_eq!(read_saved_fall_effects(&advanced).unwrap(), expected);
    }

    #[test]
    fn hunger_replay_rejects_bad_timeline_and_unrecognized_effects() {
        let original = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![]),
        };
        assert!(
            replay_saved_hunger_effects(
                &original,
                3,
                &[HungerUpdate::rotten_flesh(2), HungerUpdate::rotten_flesh(1)]
            )
            .is_err()
        );
        assert!(
            replay_saved_hunger_effects(&original, 3, &[HungerUpdate::rotten_flesh(4)]).is_err()
        );
        let invalid = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "active_effects".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:weakness".into())),
                        ("duration".into(), Tag::Int(5)),
                    ])],
                },
            )]),
        };
        assert!(
            replay_saved_hunger_effects(&invalid, 3, &[HungerUpdate::rotten_flesh(1)]).is_err()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn hunger_effect_replay_matches_java_saved_components() {
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
        let source = temp.path().join("HungerSaveOracle.java");
        std::fs::write(&source, include_str!("../tests/java/HungerSaveOracle.java")).unwrap();
        let mut input = Vec::new();
        let mut originals = vec![None];
        for duration in [5, 400, 600, 1000, -1] {
            for amplifier in [0, 1, 2] {
                for hidden in [false, true] {
                    let mut fields = vec![
                        ("id".into(), Tag::String("minecraft:hunger".into())),
                        ("duration".into(), Tag::Int(duration)),
                        ("amplifier".into(), Tag::Byte(amplifier)),
                        ("ambient".into(), Tag::Byte(1)),
                        ("show_particles".into(), Tag::Byte(0)),
                        ("show_icon".into(), Tag::Byte(0)),
                    ];
                    if hidden {
                        fields.push((
                            "hidden_effect".into(),
                            Tag::Compound(vec![
                                ("duration".into(), Tag::Int(900)),
                                ("amplifier".into(), Tag::Byte(0)),
                                ("show_particles".into(), Tag::Byte(0)),
                            ]),
                        ));
                    }
                    originals.push(Some(Tag::Compound(fields)));
                }
            }
        }
        for effect in originals {
            for (ticks, events) in [
                (40, vec![]),
                (40, vec![0]),
                (40, vec![0, 0, 5]),
                (40, vec![12, 32, 34]),
                (700, vec![3, 34]),
            ] {
                let mut initial = vec![];
                if let Some(effect) = effect.clone() {
                    initial.push(effect);
                    initial.push(Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:water_breathing".into())),
                        ("duration".into(), Tag::Int(60)),
                    ]));
                }
                let original = NamedTag {
                    name: "".into(),
                    value: Tag::Compound(
                        (!initial.is_empty())
                            .then_some((
                                "active_effects".into(),
                                Tag::List {
                                    element_type: TagType::Compound,
                                    values: initial,
                                },
                            ))
                            .into_iter()
                            .collect(),
                    ),
                };
                let updates = events
                    .iter()
                    .copied()
                    .map(HungerUpdate::rotten_flesh)
                    .collect::<Vec<_>>();
                let output = replay_saved_hunger_effects(&original, ticks, &updates).unwrap();
                let draft = NamedTag {
                    name: "".into(),
                    value: Tag::Compound(
                        output
                            .map(|value| ("active_effects".into(), value))
                            .into_iter()
                            .collect(),
                    ),
                };
                let index = input.len();
                std::fs::write(
                    temp.path().join(format!("source{index}.nbt")),
                    native_persistence::nbt::write(&original).unwrap(),
                )
                .unwrap();
                std::fs::write(
                    temp.path().join(format!("draft{index}.nbt")),
                    native_persistence::nbt::write(&draft).unwrap(),
                )
                .unwrap();
                input.push(serde_json::json!({"ticks":ticks,"events":events}));
            }
        }
        for duration in [-1, 8, 40, 800] {
            for amplifier in [0, 1, 3] {
                for visible in [false, true] {
                    let original = NamedTag {
                        name: "".into(),
                        value: Tag::Compound(vec![(
                            "active_effects".into(),
                            Tag::List {
                                element_type: TagType::Compound,
                                values: vec![Tag::Compound(vec![
                                    ("id".into(), Tag::String("minecraft:hunger".into())),
                                    ("duration".into(), Tag::Int(duration)),
                                    ("amplifier".into(), Tag::Byte(amplifier)),
                                    ("ambient".into(), Tag::Byte(1)),
                                    ("show_particles".into(), Tag::Byte(i8::from(visible))),
                                ])],
                            },
                        )]),
                    };
                    let updates = [
                        HungerUpdate {
                            tick: 0,
                            duration: 10,
                            amplifier: 2,
                            ambient: true,
                            visible: false,
                            show_icon: false,
                        },
                        HungerUpdate {
                            tick: 3,
                            duration: 200,
                            amplifier: 0,
                            ambient: false,
                            visible: true,
                            show_icon: false,
                        },
                        HungerUpdate {
                            tick: 15,
                            duration: -1,
                            amplifier: 1,
                            ambient: true,
                            visible: true,
                            show_icon: true,
                        },
                    ];
                    let effects = replay_saved_hunger_effects(&original, 50, &updates).unwrap();
                    let draft = NamedTag {
                        name: "".into(),
                        value: Tag::Compound(
                            effects
                                .map(|value| ("active_effects".into(), value))
                                .into_iter()
                                .collect(),
                        ),
                    };
                    let index = input.len();
                    std::fs::write(
                        temp.path().join(format!("source{index}.nbt")),
                        native_persistence::nbt::write(&original).unwrap(),
                    )
                    .unwrap();
                    std::fs::write(
                        temp.path().join(format!("draft{index}.nbt")),
                        native_persistence::nbt::write(&draft).unwrap(),
                    )
                    .unwrap();
                    let events: Vec<_> = updates.iter().map(|event| serde_json::json!({"tick":event.tick,"duration":event.duration,"amplifier":event.amplifier,"ambient":event.ambient,"visible":event.visible,"show_icon":event.show_icon})).collect();
                    input.push(serde_json::json!({"ticks":50,"events":events}));
                }
            }
        }
        for duration in [-1, 8, 40, 800] {
            for amplifier in [0, 1, 3] {
                for hidden in [false, true] {
                    let mut poison = vec![
                        ("id".into(), Tag::String("minecraft:poison".into())),
                        ("duration".into(), Tag::Int(duration)),
                        ("amplifier".into(), Tag::Byte(amplifier)),
                        ("ambient".into(), Tag::Byte(1)),
                    ];
                    if hidden {
                        poison.push((
                            "hidden_effect".into(),
                            Tag::Compound(vec![
                                ("duration".into(), Tag::Int(900)),
                                ("amplifier".into(), Tag::Byte(0)),
                            ]),
                        ));
                    }
                    let original = NamedTag {
                        name: "".into(),
                        value: Tag::Compound(vec![(
                            "active_effects".into(),
                            Tag::List {
                                element_type: TagType::Compound,
                                values: vec![
                                    Tag::Compound(poison),
                                    Tag::Compound(vec![
                                        ("id".into(), Tag::String("minecraft:hunger".into())),
                                        ("duration".into(), Tag::Int(40)),
                                    ]),
                                ],
                            },
                        )]),
                    };
                    let updates = [
                        StatusUpdate {
                            kind: crate::survival::FoodStatusKind::Poison,
                            remove: false,
                            clear_all: false,
                            update: HungerUpdate {
                                tick: 0,
                                duration: 10,
                                amplifier: 2,
                                ambient: true,
                                visible: false,
                                show_icon: false,
                            },
                        },
                        StatusUpdate {
                            kind: crate::survival::FoodStatusKind::Hunger,
                            remove: false,
                            clear_all: false,
                            update: HungerUpdate {
                                tick: 0,
                                duration: 100,
                                amplifier: 1,
                                ambient: false,
                                visible: true,
                                show_icon: true,
                            },
                        },
                        StatusUpdate {
                            kind: crate::survival::FoodStatusKind::Poison,
                            remove: false,
                            clear_all: false,
                            update: HungerUpdate {
                                tick: 3,
                                duration: 200,
                                amplifier: 0,
                                ambient: false,
                                visible: true,
                                show_icon: false,
                            },
                        },
                        StatusUpdate {
                            kind: crate::survival::FoodStatusKind::Poison,
                            remove: false,
                            clear_all: false,
                            update: HungerUpdate {
                                tick: 15,
                                duration: -1,
                                amplifier: 1,
                                ambient: true,
                                visible: true,
                                show_icon: true,
                            },
                        },
                    ];
                    for remove in [false, true] {
                        let mut updates = updates.to_vec();
                        if remove {
                            updates.insert(
                                3,
                                StatusUpdate {
                                    kind: crate::survival::FoodStatusKind::Poison,
                                    remove: true,
                                    clear_all: false,
                                    update: HungerUpdate {
                                        tick: 3,
                                        duration: 0,
                                        amplifier: 0,
                                        ambient: false,
                                        visible: false,
                                        show_icon: false,
                                    },
                                },
                            );
                        }
                        let effects = replay_saved_status_effects(&original, 50, &updates).unwrap();
                        let draft = NamedTag {
                            name: "".into(),
                            value: Tag::Compound(
                                effects
                                    .map(|value| ("active_effects".into(), value))
                                    .into_iter()
                                    .collect(),
                            ),
                        };
                        let index = input.len();
                        std::fs::write(
                            temp.path().join(format!("source{index}.nbt")),
                            native_persistence::nbt::write(&original).unwrap(),
                        )
                        .unwrap();
                        std::fs::write(
                            temp.path().join(format!("draft{index}.nbt")),
                            native_persistence::nbt::write(&draft).unwrap(),
                        )
                        .unwrap();
                        let events: Vec<_> = updates.iter().map(|event| {
                        let value = event.update;
                        serde_json::json!({"kind": match event.kind { crate::survival::FoodStatusKind::Poison => "poison", crate::survival::FoodStatusKind::Hunger => "hunger", crate::survival::FoodStatusKind::NightVision => "night_vision", crate::survival::FoodStatusKind::WaterBreathing => "water_breathing" }, "remove":event.remove, "tick":value.tick,"duration":value.duration,"amplifier":value.amplifier,"ambient":value.ambient,"visible":value.visible,"show_icon":value.show_icon})
                    }).collect();
                        input.push(serde_json::json!({"ticks":50,"events":events}));
                    }
                }
            }
        }
        let cases = temp.path().join("cases.json");
        std::fs::write(&cases, serde_json::to_vec(&input).unwrap()).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("HungerSaveOracle")
                .arg(temp.path())
                .arg(&cases),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java hunger save oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }

    #[test]
    fn saved_fall_effects_survive_hidden_effect_promotion_in_draft() {
        let player = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "active_effects".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![
                        Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:slow_falling".into())),
                            ("duration".into(), Tag::Int(1)),
                            (
                                "hidden_effect".into(),
                                Tag::Compound(vec![("duration".into(), Tag::Int(5))]),
                            ),
                        ]),
                        Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:levitation".into())),
                            ("duration".into(), Tag::Int(3)),
                            ("amplifier".into(), Tag::Byte(1)),
                        ]),
                        Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:night_vision".into())),
                            ("duration".into(), Tag::Int(1)),
                            (
                                "hidden_effect".into(),
                                Tag::Compound(vec![("duration".into(), Tag::Int(9))]),
                            ),
                        ]),
                        Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:dolphins_grace".into())),
                            ("duration".into(), Tag::Int(8)),
                        ]),
                        Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:hunger".into())),
                            ("duration".into(), Tag::Int(1)),
                            ("amplifier".into(), Tag::Byte(2)),
                            (
                                "hidden_effect".into(),
                                Tag::Compound(vec![
                                    ("duration".into(), Tag::Int(4)),
                                    ("amplifier".into(), Tag::Byte(1)),
                                ]),
                            ),
                        ]),
                    ],
                },
            )]),
        };
        let mut effects = read_saved_fall_effects(&player).unwrap();
        effects.tick();
        let advanced = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "active_effects".into(),
                advance_saved_effects(&player, 1).unwrap().unwrap(),
            )]),
        };
        assert_eq!(read_saved_fall_effects(&advanced).unwrap(), effects);
        assert_eq!(effects.slow_falling.unwrap().duration, 4);
        let levitation = effects.levitation.unwrap();
        assert_eq!(levitation.duration, 2);
        assert_eq!(levitation.amplifier, 1);
        assert_eq!(effects.dolphins_grace.unwrap().duration, 7);
        assert_eq!(effects.night_vision.unwrap().duration, 8);
        let hunger = effects.hunger.unwrap();
        assert_eq!((hunger.duration, hunger.amplifier), (3, 1));
    }

    #[test]
    fn effect_draft_ages_hidden_effects_and_preserves_promoted_display_settings() {
        let effect = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:water_breathing".into())),
            ("duration".into(), Tag::Int(2)),
            ("amplifier".into(), Tag::Byte(2)),
            ("show_particles".into(), Tag::Byte(1)),
            (
                "hidden_effect".into(),
                Tag::Compound(vec![
                    ("duration".into(), Tag::Int(8)),
                    ("amplifier".into(), Tag::Byte(1)),
                    ("ambient".into(), Tag::Byte(1)),
                    ("show_particles".into(), Tag::Byte(0)),
                    ("show_icon".into(), Tag::Byte(0)),
                    (
                        "hidden_effect".into(),
                        Tag::Compound(vec![("duration".into(), Tag::Int(-1))]),
                    ),
                ]),
            ),
        ]);
        let original = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "active_effects".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![effect],
                },
            )]),
        };
        let source = native_persistence::nbt::write(&original).unwrap();
        let advanced = advance_saved_effects(&original, 2).unwrap().unwrap();
        let Tag::List { values, .. } = advanced else {
            panic!()
        };
        let promoted = &values[0];
        assert_eq!(promoted.get("duration"), Some(&Tag::Int(6)));
        assert_eq!(promoted.get("amplifier"), Some(&Tag::Byte(1)));
        assert_eq!(promoted.get("ambient"), Some(&Tag::Byte(1)));
        assert_eq!(promoted.get("show_particles"), Some(&Tag::Byte(0)));
        assert_eq!(promoted.get("show_icon"), Some(&Tag::Byte(0)));
        assert_eq!(
            promoted.get("id"),
            Some(&Tag::String("minecraft:water_breathing".into()))
        );
        let Tag::List { values, .. } = advance_saved_effects(&original, u64::MAX).unwrap().unwrap()
        else {
            panic!()
        };
        assert_eq!(values[0].get("duration"), Some(&Tag::Int(-1)));
        assert_eq!(native_persistence::nbt::write(&original).unwrap(), source);
    }

    #[test]
    fn effect_draft_rejects_unsupported_or_ambiguous_data() {
        let player = |effects| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "active_effects".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: effects,
                },
            )]),
        };
        let effect = |name: &str| {
            Tag::Compound(vec![
                ("id".into(), Tag::String(name.into())),
                ("duration".into(), Tag::Int(5)),
            ])
        };
        assert!(advance_saved_effects(&player(vec![effect("minecraft:weakness")]), 1).is_err());
        // Both also change attack speed; do not write them until those changes are tracked.
        for name in ["minecraft:haste", "minecraft:mining_fatigue"] {
            assert!(advance_saved_effects(&player(vec![effect(name)]), 1).is_err());
        }
        assert!(
            advance_saved_effects(
                &player(vec![effect("minecraft:resistance"), effect("resistance")]),
                1
            )
            .is_err()
        );
        let mut unknown = effect("minecraft:resistance");
        set_field(&mut unknown, "future_data", Some(Tag::Int(1))).unwrap();
        assert!(advance_saved_effects(&player(vec![unknown]), 1).is_err());
        let mut duplicate = effect("minecraft:resistance");
        if let Tag::Compound(entries) = &mut duplicate {
            entries.push(("duration".into(), Tag::Int(3)));
        }
        assert!(advance_saved_effects(&player(vec![duplicate]), 1).is_err());
        let malformed = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("active_effects".into(), Tag::Int(0))]),
        };
        assert!(advance_saved_effects(&malformed, 1).is_err());
    }

    #[test]
    fn flight_draft_preserves_other_abilities_and_rejects_ambiguous_data() {
        let mut original = player_with_inventory();
        let abilities = Tag::Compound(vec![
            ("flying".into(), Tag::Byte(0)),
            ("flySpeed".into(), Tag::Float(0.125)),
            ("unknown".into(), Tag::String("keep".into())),
        ]);
        set_field(&mut original.value, "abilities", Some(abilities.clone())).unwrap();
        let inventory = read_saved_player(&original).unwrap();
        let mut draft = PlayerStateDraft(draft_player_inventory(&original, &inventory).unwrap());
        draft.set_flying(&original, true).unwrap();
        assert!(read_saved_flying(draft.document()));
        let result = draft.document().value.get("abilities").unwrap();
        assert_eq!(result.get("flySpeed"), Some(&Tag::Float(0.125)));
        assert_eq!(result.get("unknown"), Some(&Tag::String("keep".into())));
        assert_eq!(original.value.get("abilities"), Some(&abilities));
        assert!(draft.matches_source(&original).unwrap());
        let mut duplicate = original.clone();
        if let Tag::Compound(entries) = &mut duplicate.value {
            entries.push(("abilities".into(), abilities));
        }
        assert!(draft.set_flying(&duplicate, true).is_err());
        let mut invalid = original.clone();
        set_field(
            &mut invalid.value,
            "abilities",
            Some(Tag::String("invalid".into())),
        )
        .unwrap();
        assert!(draft.set_flying(&invalid, true).is_err());
    }

    #[test]
    fn last_death_draft_preserves_unrelated_fields_and_rejects_unknown_target_data() {
        let original = player_with_inventory();
        let inventory = read_saved_player(&original).unwrap();
        let mut draft = PlayerStateDraft(draft_player_inventory(&original, &inventory).unwrap());
        let target = crate::lodestone::Target {
            dimension: ResourceId::parse("example:dimension").unwrap(),
            position: glam::IVec3::new(-30, -64, 42),
        };
        assert_eq!(read_last_death_location(&original).unwrap(), None);
        draft.set_last_death_location(&original, &target).unwrap();
        assert_eq!(
            read_last_death_location(draft.document()).unwrap(),
            Some(target.clone())
        );
        assert_eq!(
            draft.document().value.get("custom_world_data"),
            original.value.get("custom_world_data")
        );
        assert!(draft.matches_source(&original).unwrap());
        let mut unknown = original.clone();
        set_field(
            &mut unknown.value,
            "LastDeathLocation",
            Some(Tag::Compound(vec![
                (
                    "dimension".into(),
                    Tag::String("minecraft:overworld".into()),
                ),
                ("pos".into(), Tag::IntArray(vec![0, 0, 0])),
                ("future".into(), Tag::Int(1)),
            ])),
        )
        .unwrap();
        assert!(read_last_death_location(&unknown).is_err());
        assert!(draft.set_last_death_location(&unknown, &target).is_err());
    }

    #[test]
    fn player_state_draft_preserves_unknown_data_and_round_trips_simulated_fields() {
        let original = player_with_inventory();
        let before = native_persistence::nbt::write(&original).unwrap();
        let mut inventory = read_saved_player(&original).unwrap();
        let item = inventory.slot(PlayerSlot::Storage(0)).cloned().unwrap();
        inventory.set_slot(PlayerSlot::Storage(0), None);
        inventory.set_slot(PlayerSlot::Storage(8), Some(item.clone()));
        inventory.select(8);
        let state = PlayerSaveState {
            position: DVec3::new(16_000_000.123_456_79, 72.125, -16_000_000.987_654_32),
            motion: SavedPlayerMotion {
                velocity: DVec3::new(0.125, -0.2, 0.375),
                on_ground: false,
                fall_distance: 2.75,
            },
            yaw: 123.5,
            pitch: -24.25,
            dimension: "minecraft:overworld".into(),
            game_mode: GameMode::Survival,
            experience: crate::experience::Experience {
                progress: 0.375,
                level: 18,
                total: 321,
            },
            score: 321,
            vitals: crate::survival::PlayerVitals {
                health: 14.0,
                absorption: 1.5,
                air: -12,
                food: 12,
                saturation: 2.25,
                exhaustion: 3.5,
                food_tick_timer: 29,
                ..Default::default()
            },
        };
        let draft = draft_player_state(&original, &state, &inventory).unwrap();
        assert!(draft.matches_source(&original).unwrap());
        assert_eq!(before, native_persistence::nbt::write(&original).unwrap());
        let result = draft.document();
        assert_eq!(
            result.value.get("custom_world_data"),
            original.value.get("custom_world_data")
        );
        assert_eq!(
            result.value.get("equipment").unwrap().get("body"),
            original.value.get("equipment").unwrap().get("body")
        );
        assert_eq!(read_saved_position(result).unwrap(), Some(state.position));
        assert_eq!(read_saved_motion(result).unwrap(), state.motion);
        assert_eq!(read_saved_vitals(result, false).unwrap(), state.vitals);
        assert_eq!(read_saved_experience(result), state.experience);
        assert_eq!(result.value.get("Score"), Some(&Tag::Int(state.score)));
        assert_eq!(result.value.get("Air"), Some(&Tag::Short(-12)));
        assert_eq!(
            result.value.get("Rotation"),
            Some(&Tag::List {
                element_type: TagType::Float,
                values: vec![Tag::Float(state.yaw), Tag::Float(state.pitch)]
            })
        );
        assert_eq!(
            read_saved_player(result)
                .unwrap()
                .slot(PlayerSlot::Storage(8)),
            Some(&item)
        );
        let encoded = native_persistence::nbt::write(result).unwrap();
        assert_eq!(native_persistence::nbt::read(&encoded).unwrap(), *result);
        let mut changed = original.clone();
        set_field(&mut changed.value, "other", Some(Tag::Int(1))).unwrap();
        assert!(!draft.matches_source(&changed).unwrap());
    }

    #[test]
    fn player_draft_keeps_maximum_health_attributes_and_validates_them() {
        let mut original = player_with_inventory();
        let attributes = Tag::List {
            element_type: TagType::Compound,
            values: vec![Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:max_health".into())),
                ("base".into(), Tag::Double(20.0)),
                (
                    "modifiers".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("example:bonus".into())),
                            ("amount".into(), Tag::Double(5.0)),
                            ("operation".into(), Tag::String("add_value".into())),
                        ])],
                    },
                ),
                ("future".into(), Tag::Int(3)),
            ])],
        };
        set_field(&mut original.value, "attributes", Some(attributes.clone())).unwrap();
        let inventory = read_saved_player(&original).unwrap();
        let state = PlayerSaveState {
            position: DVec3::new(0.0, 70.0, 0.0),
            motion: SavedPlayerMotion {
                velocity: DVec3::ZERO,
                on_ground: false,
                fall_distance: 0.0,
            },
            yaw: 0.0,
            pitch: 0.0,
            dimension: "minecraft:overworld".into(),
            game_mode: GameMode::Survival,
            experience: Default::default(),
            score: 0,
            vitals: crate::survival::PlayerVitals {
                health: 23.0,
                max_health: 25.0,
                ..Default::default()
            },
        };
        let draft = draft_player_state(&original, &state, &inventory).unwrap();
        assert_eq!(draft.document().value.get("attributes"), Some(&attributes));
        assert_eq!(
            read_saved_vitals(draft.document(), false).unwrap(),
            state.vitals
        );
        let mut invalid = state.clone();
        invalid.vitals.max_health = 20.0;
        assert!(draft_player_state(&original, &invalid, &inventory).is_err());
    }

    #[test]
    fn player_state_draft_rejects_ambiguous_sources_and_values_that_would_not_reload() {
        let original = player_with_inventory();
        let inventory = read_saved_player(&original).unwrap();
        let state = PlayerSaveState {
            position: DVec3::new(1.0, 70.0, 2.0),
            motion: SavedPlayerMotion {
                velocity: DVec3::ZERO,
                on_ground: true,
                fall_distance: 0.0,
            },
            yaw: 0.0,
            pitch: 0.0,
            dimension: "minecraft:overworld".into(),
            game_mode: GameMode::Survival,
            experience: Default::default(),
            score: 0,
            vitals: Default::default(),
        };
        let mut malformed = original.clone();
        set_field(&mut malformed.value, "XpP", Some(Tag::Float(f32::NAN))).unwrap();
        assert!(draft_player_state(&malformed, &state, &inventory).is_err());
        set_field(&mut malformed.value, "XpP", None).unwrap();
        set_field(
            &mut malformed.value,
            "Pos",
            Some(Tag::LongArray(vec![1, 2, 3])),
        )
        .unwrap();
        assert!(draft_player_state(&malformed, &state, &inventory).is_err());
        set_field(
            &mut malformed.value,
            "Pos",
            Some(Tag::List {
                element_type: TagType::Double,
                values: vec![
                    Tag::Double(30_000_513.0),
                    Tag::Double(2.0),
                    Tag::Double(3.0),
                ],
            }),
        )
        .unwrap();
        assert!(draft_player_state(&malformed, &state, &inventory).is_err());
        let mut bad_vitals = original.clone();
        set_field(&mut bad_vitals.value, "Air", Some(Tag::Int(1))).unwrap();
        assert!(draft_player_state(&bad_vitals, &state, &inventory).is_err());
        set_field(&mut bad_vitals.value, "Air", None).unwrap();
        let mut unrepresentable_air = state.clone();
        unrepresentable_air.vitals.air = i16::MAX as i32 + 1;
        assert!(draft_player_state(&original, &unrepresentable_air, &inventory).is_err());
        set_field(&mut bad_vitals.value, "Health", Some(Tag::Float(21.0))).unwrap();
        assert!(draft_player_state(&bad_vitals, &state, &inventory).is_err());
        set_field(&mut bad_vitals.value, "Health", Some(Tag::Float(20.0))).unwrap();
        set_field(
            &mut bad_vitals.value,
            "AbsorptionAmount",
            Some(Tag::Float(-1.0)),
        )
        .unwrap();
        assert!(draft_player_state(&bad_vitals, &state, &inventory).is_err());
        let mut bad_attributes = original.clone();
        set_field(
            &mut bad_attributes.value,
            "attributes",
            Some(Tag::String("unknown".into())),
        )
        .unwrap();
        assert!(draft_player_state(&bad_attributes, &state, &inventory).is_err());
        let modifier = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:example".into())),
            ("amount".into(), Tag::Double(1.0)),
            ("operation".into(), Tag::String("add_value".into())),
        ]);
        set_field(
            &mut bad_attributes.value,
            "attributes",
            Some(Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:max_health".into())),
                    ("base".into(), Tag::Double(20.0)),
                    (
                        "modifiers".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![modifier.clone(), modifier],
                        },
                    ),
                ])],
            }),
        )
        .unwrap();
        assert!(draft_player_state(&bad_attributes, &state, &inventory).is_err());
        set_field(
            &mut malformed.value,
            "Pos",
            Some(Tag::List {
                element_type: TagType::Double,
                values: vec![Tag::Double(f64::NAN), Tag::Double(2.0), Tag::Double(3.0)],
            }),
        )
        .unwrap();
        assert!(draft_player_state(&malformed, &state, &inventory).is_err());
        let mut duplicate = original.clone();
        if let Tag::Compound(fields) = &mut duplicate.value {
            fields.push(("OnGround".into(), Tag::Byte(0)));
            fields.push(("OnGround".into(), Tag::Byte(1)));
        }
        assert!(draft_player_state(&duplicate, &state, &inventory).is_err());
        let mut changed = state.clone();
        changed.motion.velocity.x = 10.001;
        assert!(draft_player_state(&original, &changed, &inventory).is_err());
        changed = state.clone();
        changed.position.x = 30_000_512.5;
        assert!(draft_player_state(&original, &changed, &inventory).is_err());
        changed = state.clone();
        changed.vitals.max_health = 30.0;
        assert!(draft_player_state(&original, &changed, &inventory).is_err());
        let mut other_dimension = original.clone();
        set_field(
            &mut other_dimension.value,
            "Dimension",
            Some(Tag::String("minecraft:the_nether".into())),
        )
        .unwrap();
        assert!(draft_player_state(&other_dimension, &state, &inventory).is_err());
        let mut other_mode = original.clone();
        set_field(&mut other_mode.value, "playerGameType", Some(Tag::Int(1))).unwrap();
        assert!(draft_player_state(&other_mode, &state, &inventory).is_err());
    }

    #[test]
    fn saved_vitals_use_food_defaults_and_max_health_attributes() {
        let modifier = |id: &str, operation: &str, amount: f64| {
            Tag::Compound(vec![
                ("id".into(), Tag::String(id.into())),
                ("operation".into(), Tag::String(operation.into())),
                ("amount".into(), Tag::Double(amount)),
            ])
        };
        let player = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("Health".into(), Tag::Float(47.0)),
                ("foodLevel".into(), Tag::Int(6)),
                (
                    "attributes".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:max_health".into())),
                            ("base".into(), Tag::Double(20.0)),
                            (
                                "modifiers".into(),
                                Tag::List {
                                    element_type: TagType::Compound,
                                    values: vec![
                                        modifier("example:a", "add_value", 4.0),
                                        modifier("example:b", "add_multiplied_base", 0.5),
                                        modifier("example:c", "add_multiplied_total", 0.25),
                                    ],
                                },
                            ),
                        ])],
                    },
                ),
            ]),
        };
        let vitals = read_saved_vitals(&player, true).unwrap();
        assert_eq!(vitals.max_health, 45.0);
        assert_eq!(vitals.health, 45.0);
        assert_eq!(vitals.food, 6);
        assert_eq!(vitals.saturation, 5.0);
        assert!(vitals.hardcore);
        assert!(!vitals.can_sprint(GameMode::Survival));
        assert!(vitals.can_sprint(GameMode::Creative));
    }

    #[test]
    fn saved_motion_uses_java_defaults_numeric_values_and_limits() {
        let player = |fields: Vec<(&str, Tag)>| NamedTag {
            name: "".into(),
            value: Tag::Compound(fields.into_iter().map(|(k, v)| (k.into(), v)).collect()),
        };
        let parsed = read_saved_motion(&player(vec![
            (
                "Motion",
                Tag::List {
                    element_type: TagType::Double,
                    values: vec![Tag::Double(10.0), Tag::Double(-10.01), Tag::Double(0.125)],
                },
            ),
            ("OnGround", Tag::Double(-0.5)),
            ("fall_distance", Tag::Float(3.25)),
        ]))
        .unwrap();
        assert_eq!(parsed.velocity, DVec3::new(10.0, 0.0, 0.125));
        assert!(parsed.on_ground);
        assert_eq!(parsed.fall_distance, 3.25);
        let parsed = read_saved_motion(&player(vec![
            ("Motion", Tag::IntArray(vec![3, -2, 1, 9])),
            ("OnGround", Tag::Int(256)),
        ]))
        .unwrap();
        assert_eq!(parsed.velocity, DVec3::new(3.0, -2.0, 1.0));
        assert!(!parsed.on_ground);
        assert_eq!(parsed.fall_distance, 0.0);
        let parsed = read_saved_motion(&player(vec![(
            "Motion",
            Tag::List {
                element_type: TagType::Double,
                values: vec![Tag::Double(1.0), Tag::Double(2.0)],
            },
        )]))
        .unwrap();
        assert_eq!(parsed.velocity, DVec3::ZERO);
        assert!(
            read_saved_motion(&player(vec![("fall_distance", Tag::Double(f64::NAN))])).is_err()
        );
    }

    fn player_with_inventory() -> NamedTag {
        NamedTag {
            name: "player".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("health".into(), Tag::Float(17.0)),
                (
                    "custom_world_data".into(),
                    Tag::Compound(vec![("opaque".into(), Tag::Long(42))]),
                ),
                ("SelectedItemSlot".into(), Tag::Int(0)),
                (
                    "Inventory".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![
                            item("minecraft:stone", 0, 5, Tag::Compound(vec![])),
                            item(
                                "minecraft:bundle",
                                4,
                                1,
                                Tag::Compound(vec![(
                                    "minecraft:custom_data".into(),
                                    Tag::Compound(vec![("x".into(), Tag::Int(9))]),
                                )]),
                            ),
                        ],
                    },
                ),
                (
                    "equipment".into(),
                    Tag::Compound(vec![
                        (
                            "offhand".into(),
                            Tag::Compound(vec![
                                ("id".into(), Tag::String("minecraft:shield".into())),
                                ("count".into(), Tag::Int(1)),
                            ]),
                        ),
                        (
                            "body".into(),
                            Tag::Compound(vec![("modded".into(), Tag::Int(3))]),
                        ),
                    ]),
                ),
                (
                    "recipeBook".into(),
                    Tag::Compound(vec![
                        (
                            "recipes".into(),
                            Tag::List {
                                element_type: TagType::String,
                                values: vec![Tag::String("minecraft:stick".into())],
                            },
                        ),
                        (
                            "toBeDisplayed".into(),
                            Tag::List {
                                element_type: TagType::String,
                                values: vec![],
                            },
                        ),
                        ("isGuiOpen".into(), Tag::Byte(1)),
                        ("isFilteringCraftable".into(), Tag::Byte(0)),
                        ("isFurnaceGuiOpen".into(), Tag::Byte(1)),
                        ("future".into(), Tag::String("preserved".into())),
                    ]),
                ),
            ]),
        }
    }

    #[test]
    fn player_plan_rejects_missing_malformed_and_duplicate_identity() {
        let uuid = [1, 2, 3, 4];
        for tags in [
            vec![],
            vec![("UUID".into(), Tag::IntArray(vec![1, 2, 3]))],
            vec![
                ("UUID".into(), Tag::IntArray(uuid.to_vec())),
                ("UUID".into(), Tag::IntArray(uuid.to_vec())),
            ],
        ] {
            let mut source = player_with_inventory();
            let Tag::Compound(fields) = &mut source.value else {
                unreachable!()
            };
            fields.extend(tags);
            let inventory = read_saved_player(&source).unwrap();
            let draft = PlayerStateDraft(draft_player_inventory(&source, &inventory).unwrap());
            assert!(
                draft
                    .add_to_save_plan(
                        &mut native_persistence::save::WorldSavePlan::new(),
                        uuid,
                        &source
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn drafts_inventory_and_recipe_changes_without_losing_other_player_data() {
        let original = player_with_inventory();
        let before = original.clone();
        let mut inventory = read_saved_player(&original).unwrap();
        let bundle = inventory.slot(PlayerSlot::Storage(4)).cloned().unwrap();
        inventory.set_slot(PlayerSlot::Storage(4), None);
        inventory.set_slot(PlayerSlot::Storage(23), Some(bundle.clone()));
        inventory.set_slot(PlayerSlot::Storage(0), None);
        inventory.set_slot(
            PlayerSlot::Storage(8),
            ItemStack::new("minecraft:stone".into(), 3, 64, Tag::Compound(vec![])),
        );
        inventory.select(8);
        let mut book = inventory.recipe_book().clone();
        book.open = false;
        book.known
            .insert(ResourceId::parse("minecraft:oak_planks").unwrap());
        book.highlighted
            .insert(ResourceId::parse("minecraft:oak_planks").unwrap());
        inventory.set_recipe_book(book.clone());

        let draft = draft_player_inventory(&original, &inventory).unwrap();
        let document = draft.document();
        assert!(draft.matches_source(&original).unwrap());
        let mut concurrently_changed = original.clone();
        set_field(
            &mut concurrently_changed.value,
            "health",
            Some(Tag::Float(18.0)),
        )
        .unwrap();
        assert!(!draft.matches_source(&concurrently_changed).unwrap());
        assert_eq!(original, before);
        assert_eq!(document.value.get("health"), original.value.get("health"));
        assert_eq!(
            document.value.get("custom_world_data"),
            original.value.get("custom_world_data")
        );
        assert_eq!(
            document.value.get("equipment").unwrap().get("body"),
            original.value.get("equipment").unwrap().get("body")
        );
        assert_eq!(
            document.value.get("recipeBook").unwrap().get("future"),
            original.value.get("recipeBook").unwrap().get("future")
        );
        assert_eq!(
            document
                .value
                .get("recipeBook")
                .unwrap()
                .get("isFurnaceGuiOpen"),
            Some(&Tag::Byte(1))
        );
        let reread = read_saved_player(document).unwrap();
        assert_eq!(reread.selected(), 8);
        assert_eq!(reread.slot(PlayerSlot::Storage(23)), Some(&bundle));
        assert_eq!(reread.slot(PlayerSlot::Storage(8)).unwrap().count(), 3);
        assert_eq!(reread.recipe_book(), &book);
        let encoded = native_persistence::nbt::write(document).unwrap();
        assert_eq!(&native_persistence::nbt::read(&encoded).unwrap(), document);
    }

    #[test]
    fn inventory_draft_rejects_unrepresentable_or_ambiguous_data() {
        let original = player_with_inventory();
        let inventory = read_saved_player(&original).unwrap();

        let mut open = inventory.clone();
        assert!(open.toggle());
        assert!(draft_player_inventory(&original, &open).is_err());

        let mut missing = inventory.clone();
        missing.set_slot(PlayerSlot::Storage(4), None);
        assert!(draft_player_inventory(&original, &missing).is_err());
        let mut duplicate = inventory.clone();
        duplicate.set_slot(
            PlayerSlot::Storage(5),
            inventory.slot(PlayerSlot::Storage(4)).cloned(),
        );
        assert!(draft_player_inventory(&original, &duplicate).is_err());
        let mut oversized = inventory.clone();
        oversized.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:stone".into(), 100, 64, Tag::Compound(vec![])),
        );
        assert!(draft_player_inventory(&original, &oversized).is_err());

        let mut bad_version = original.clone();
        set_field(&mut bad_version.value, "DataVersion", Some(Tag::Int(5022))).unwrap();
        assert!(draft_player_inventory(&bad_version, &inventory).is_err());
        let mut bad_stack = original.clone();
        let Tag::Compound(fields) = &mut bad_stack.value else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields
            .iter_mut()
            .find(|(name, _)| name.matches("Inventory"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(stack) = &mut values[0] else {
            unreachable!()
        };
        stack.push(("future_item_data".into(), Tag::Int(7)));
        assert!(draft_player_inventory(&bad_stack, &inventory).is_err());
        let mut duplicate_slot = original.clone();
        let Tag::Compound(fields) = &mut duplicate_slot.value else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields
            .iter_mut()
            .find(|(name, _)| name.matches("Inventory"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        values.push(values[0].clone());
        assert!(draft_player_inventory(&duplicate_slot, &inventory).is_err());
    }

    #[test]
    fn unchanged_recipe_data_is_preserved_but_ambiguous_changes_are_rejected() {
        let mut original = player_with_inventory();
        let book = original.value.get("recipeBook").cloned().unwrap();
        let mut duplicate_book = book.clone();
        set_field(
            &mut duplicate_book,
            "recipes",
            Some(Tag::List {
                element_type: TagType::String,
                values: vec![
                    Tag::String("stick".into()),
                    Tag::String("minecraft:stick".into()),
                ],
            }),
        )
        .unwrap();
        set_field(
            &mut original.value,
            "recipeBook",
            Some(duplicate_book.clone()),
        )
        .unwrap();
        let mut inventory = read_saved_player(&original).unwrap();
        let draft = draft_player_inventory(&original, &inventory).unwrap();
        assert_eq!(
            draft.document().value.get("recipeBook"),
            Some(&duplicate_book)
        );
        inventory.set_recipe_book_settings(false, true);
        assert!(draft_player_inventory(&original, &inventory).is_err());
    }

    fn item(id: &str, slot: i8, count: i32, components: Tag) -> Tag {
        Tag::Compound(vec![
            ("Slot".into(), Tag::Byte(slot)),
            ("id".into(), Tag::String(id.into())),
            ("count".into(), Tag::Int(count)),
            ("components".into(), components),
        ])
    }

    #[test]
    fn reads_storage_selected_slot_and_equipment_without_losing_patches() {
        let patch = Tag::Compound(vec![(
            "minecraft:custom_data".into(),
            Tag::Compound(vec![("x".into(), Tag::Int(7))]),
        )]);
        let player = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("SelectedItemSlot".into(), Tag::Int(2)),
                (
                    "Inventory".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![
                            item("minecraft:stone", 2, 12, Tag::Compound(vec![])),
                            item("minecraft:diamond_sword", 3, 1, patch.clone()),
                        ],
                    },
                ),
                (
                    "equipment".into(),
                    Tag::Compound(vec![(
                        "offhand".into(),
                        item("minecraft:shield", 0, 1, Tag::Compound(vec![])),
                    )]),
                ),
            ]),
        };
        let mut inventory = read_saved_player(&player).unwrap();
        assert_eq!(inventory.selected(), 2);
        assert_eq!(inventory.slot(PlayerSlot::Storage(2)).unwrap().count(), 12);
        assert_eq!(
            inventory.slot(PlayerSlot::Storage(2)).unwrap().max_count(),
            Some(64)
        );
        let sword = inventory.slot(PlayerSlot::Storage(3)).unwrap();
        assert_eq!(sword.max_count(), Some(1));
        assert_eq!(sword.components(), &patch);
        assert_eq!(
            inventory.slot(PlayerSlot::Offhand).unwrap().id(),
            "minecraft:shield"
        );
        assert!(inventory.toggle());
        assert!(inventory.click(
            PlayerSlot::Storage(3),
            crate::inventory::ClickButton::Primary
        ));
        assert!(inventory.click(
            PlayerSlot::Storage(4),
            crate::inventory::ClickButton::Primary
        ));
        assert_eq!(
            inventory.slot(PlayerSlot::Storage(4)).unwrap().components(),
            &patch
        );
    }

    #[test]
    fn rejects_invalid_selected_slot_and_stack_shape() {
        let invalid = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("SelectedItemSlot".into(), Tag::Int(9))]),
        };
        assert!(read_saved_player(&invalid).is_err());
        assert!(read_stack(&Tag::Compound(vec![("count".into(), Tag::Int(0))])).is_err());
    }

    #[test]
    fn malformed_or_missing_recipe_book_is_ignored_like_server_player() {
        let invalid = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "recipeBook".into(),
                Tag::Compound(vec![(
                    "recipes".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: vec![Tag::String("minecraft:stick".into())],
                    },
                )]),
            )]),
        };
        assert_eq!(
            read_saved_player(&invalid).unwrap().recipe_book(),
            &RecipeBookState::default()
        );
    }

    #[test]
    fn known_stack_limit_patch_is_resolved_without_discarding_nbt() {
        let patch = Tag::Compound(vec![("minecraft:max_stack_size".into(), Tag::Int(99))]);
        let saved = read_stack(&item("minecraft:stone", 1, 99, patch.clone())).unwrap();
        assert_eq!(saved.max_count(), Some(99));
        assert_eq!(saved.components(), &patch);
        let known = Tag::Compound(vec![
            ("minecraft:max_stack_size".into(), Tag::Int(99)),
            ("minecraft:custom_data".into(), Tag::Compound(vec![])),
        ]);
        assert_eq!(
            read_stack(&item("minecraft:stone", 1, 99, known))
                .unwrap()
                .max_count(),
            Some(99)
        );
        let unknown = Tag::Compound(vec![("minecraft:equippable".into(), Tag::Compound(vec![]))]);
        assert_eq!(
            read_stack(&item("minecraft:stone", 1, 1, unknown))
                .unwrap()
                .max_count(),
            None
        );
        let damage = Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(24))]);
        let sword = read_stack(&item("minecraft:diamond_sword", 1, 1, damage.clone())).unwrap();
        assert_eq!(sword.max_count(), Some(1));
        assert_eq!(sword.components(), &damage);
        let enchanted = Tag::Compound(vec![
            ("minecraft:damage".into(), Tag::Int(24)),
            (
                "minecraft:enchantments".into(),
                Tag::Compound(vec![("minecraft:mending".into(), Tag::Int(1))]),
            ),
        ]);
        let tool = read_stack(&item("minecraft:diamond_pickaxe", 1, 1, enchanted.clone())).unwrap();
        assert_eq!(tool.max_count(), Some(1));
        assert_eq!(tool.components(), &enchanted);
    }

    #[test]
    fn saved_glint_overrides_remain_interactive_and_preserve_components() {
        for (id, patch, expected_max, expected_glint) in [
            (
                "minecraft:stone",
                Tag::Compound(vec![(
                    "minecraft:enchantment_glint_override".into(),
                    Tag::Byte(1),
                )]),
                64,
                true,
            ),
            (
                "minecraft:enchanted_golden_apple",
                Tag::Compound(vec![(
                    "!minecraft:enchantment_glint_override".into(),
                    Tag::Compound(vec![]),
                )]),
                64,
                false,
            ),
            (
                "minecraft:diamond_helmet",
                Tag::Compound(vec![
                    (
                        "minecraft:enchantments".into(),
                        Tag::Compound(vec![("minecraft:mending".into(), Tag::Int(1))]),
                    ),
                    ("minecraft:enchantment_glint_override".into(), Tag::Byte(0)),
                ]),
                1,
                false,
            ),
        ] {
            let saved = read_stack(&item(id, 0, 1, patch.clone())).unwrap();
            assert_eq!(saved.max_count(), Some(expected_max), "{id}");
            assert_eq!(saved.has_glint(), expected_glint, "{id}");
            assert_eq!(saved.components(), &patch);
            let mut slot = Some(saved.clone());
            let mut cursor = None;
            assert!(crate::inventory::pickup(
                &mut slot,
                &mut cursor,
                crate::inventory::ClickButton::Primary,
                crate::inventory::SlotRules::STANDARD
            ));
            assert_eq!(cursor, Some(saved));
        }
        for patch in [
            Tag::Compound(vec![(
                "minecraft:enchantment_glint_override".into(),
                Tag::Int(1),
            )]),
            Tag::Compound(vec![
                ("minecraft:enchantment_glint_override".into(), Tag::Byte(1)),
                (
                    "!minecraft:enchantment_glint_override".into(),
                    Tag::Compound(vec![]),
                ),
            ]),
        ] {
            assert_eq!(
                read_stack(&item("minecraft:stone", 0, 1, patch))
                    .unwrap()
                    .max_count(),
                None
            );
        }
    }

    #[test]
    fn reads_the_player_mode_and_leaves_missing_values_for_world_fallback() {
        let player = |value: Option<Tag>| NamedTag {
            name: "".into(),
            value: Tag::Compound(
                value
                    .into_iter()
                    .map(|tag| ("playerGameType".into(), tag))
                    .collect(),
            ),
        };
        assert_eq!(saved_game_mode(&player(None)), None);
        assert_eq!(
            saved_game_mode(&player(Some(Tag::String("creative".into())))),
            None
        );
        assert_eq!(
            saved_game_mode(&player(Some(Tag::Int(1)))),
            Some(GameMode::Creative)
        );
        assert_eq!(
            saved_game_mode(&player(Some(Tag::Byte(2)))),
            Some(GameMode::Adventure)
        );
        assert_eq!(
            saved_game_mode(&player(Some(Tag::Int(99)))),
            Some(GameMode::Survival)
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn reads_player_inventory_encoded_by_vanilla_26_3() {
        use std::{
            ffi::OsString,
            fs,
            path::{Path, PathBuf},
            process::Command,
        };
        const JAVA: &str = r#"
import java.io.DataInput;
import java.io.DataInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.Registries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.stats.RecipeBookSettings;
import net.minecraft.stats.ServerRecipeBook;
import net.minecraft.world.ItemStackWithSlot;
import net.minecraft.world.entity.EntityEquipment;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.component.CustomData;
import net.minecraft.world.inventory.RecipeBookType;
import java.util.List;

public class InventorySaveOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Items.STONE.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
        Items.DIAMOND_SWORD.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,1).build());
        Items.SHIELD.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,1).build());
        if (args.length > 1) {
            CompoundTag saved;
            try (var input = new DataInputStream(Files.newInputStream(Path.of(args[0])))) {
                saved = NbtIo.read((DataInput)input);
            }
            if (saved.getIntOr("SelectedItemSlot", -1) != 4) throw new AssertionError("selected slot");
            var slots = (ListTag)saved.get("Inventory");
            boolean stone = false, shield = false, sword = false, large = false;
            for (var tag : slots) {
                var entry = ItemStackWithSlot.CODEC.parse(NbtOps.INSTANCE, tag).getOrThrow();
                var stack = entry.stack();
                if (entry.slot() == 3) stone = stack.is(Items.STONE) && stack.getCount() == 8;
                if (entry.slot() == 4) large = stack.is(Items.STONE) && stack.getCount() == 99 && stack.getMaxStackSize() == 99;
                if (entry.slot() == 5) shield = stack.is(Items.SHIELD) && stack.getCount() == 1;
                if (entry.slot() == 30) sword = stack.is(Items.DIAMOND_SWORD) && stack.getCount() == 1
                    && stack.get(DataComponents.CUSTOM_DATA).copyTag().getIntOr("marker", -1) == 7654;
                if (entry.slot() == 14) throw new AssertionError("old sword slot still present");
            }
            if (!stone || !shield || !sword || !large) throw new AssertionError("saved inventory entries");
            var gear = EntityEquipment.CODEC.parse(NbtOps.INSTANCE, saved.get("equipment")).getOrThrow();
            if (!gear.get(EquipmentSlot.OFFHAND).is(Items.STONE) || gear.get(EquipmentSlot.OFFHAND).getCount() != 7)
                throw new AssertionError("offhand");
            if (!gear.get(EquipmentSlot.HEAD).is(Items.DIAMOND_SWORD)) throw new AssertionError("head equipment");
            var book = ServerRecipeBook.Packed.CODEC.parse(NbtOps.INSTANCE, saved.get("recipeBook")).getOrThrow();
            var torch = ResourceKey.create(Registries.RECIPE, Identifier.withDefaultNamespace("torch"));
            if (!book.known().contains(torch) || !book.highlight().contains(torch)
                || book.settings().isOpen(RecipeBookType.CRAFTING)
                || !book.settings().isFiltering(RecipeBookType.CRAFTING)) throw new AssertionError("recipe book");
            if (saved.getIntOr("unknown_value", -1) != 9182) throw new AssertionError("unknown field");
            return;
        }
        ItemStack sword = new ItemStack(Items.DIAMOND_SWORD, 1);
        ItemStack large = new ItemStack(Items.STONE, 99);
        large.set(DataComponents.MAX_STACK_SIZE, 99);
        CompoundTag custom = new CompoundTag(); custom.putInt("marker", 7654);
        sword.set(DataComponents.CUSTOM_DATA, CustomData.of(custom));
        ListTag slots = new ListTag();
        slots.add(ItemStackWithSlot.CODEC.encodeStart(NbtOps.INSTANCE, new ItemStackWithSlot(3,new ItemStack(Items.STONE,5))).getOrThrow());
        slots.add(ItemStackWithSlot.CODEC.encodeStart(NbtOps.INSTANCE, new ItemStackWithSlot(4,large)).getOrThrow());
        slots.add(ItemStackWithSlot.CODEC.encodeStart(NbtOps.INSTANCE, new ItemStackWithSlot(14,sword)).getOrThrow());
        EntityEquipment gear = new EntityEquipment(); gear.set(EquipmentSlot.OFFHAND, new ItemStack(Items.SHIELD,1));
        CompoundTag player = new CompoundTag(); player.putInt("DataVersion", 5023); player.putInt("unknown_value", 9182); player.put("Inventory", slots); player.putInt("SelectedItemSlot",3);
        player.put("equipment", EntityEquipment.CODEC.encodeStart(NbtOps.INSTANCE,gear).getOrThrow());
        var settings = new RecipeBookSettings();
        settings.setOpen(RecipeBookType.CRAFTING,true); settings.setFiltering(RecipeBookType.CRAFTING,true);
        var planks = ResourceKey.create(Registries.RECIPE,Identifier.withDefaultNamespace("oak_planks"));
        var stick = ResourceKey.create(Registries.RECIPE,Identifier.withDefaultNamespace("stick"));
        player.put("recipeBook",ServerRecipeBook.Packed.CODEC.encodeStart(NbtOps.INSTANCE,
            new ServerRecipeBook.Packed(settings,List.of(planks,stick),List.of(stick))).getOrThrow());
        NbtIo.writeCompressed(player, Path.of(args[0]));
    }
}
"#;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("InventorySaveOracle.java");
        let saved = temp.path().join("player.dat");
        fs::write(&source, JAVA).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(game.join("versions/26.3/26.3.json")).unwrap())
                .unwrap();
        let mut classpath = vec![
            game.join("versions/26.3/26.3.jar"),
            temp.path().to_path_buf(),
        ];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    classpath.push(path);
                }
            }
        }
        let classpath: OsString = std::env::join_paths(classpath).unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "Java inventory save oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("InventorySaveOracle")
            .arg(&saved));
        let player =
            native_persistence::nbt::read_gzip(fs::File::open(Path::new(&saved)).unwrap()).unwrap();
        let inventory = read_saved_player(&player).unwrap();
        assert_eq!(inventory.selected(), 3);
        let stone = inventory.slot(PlayerSlot::Storage(3)).unwrap();
        assert_eq!(
            (stone.id(), stone.count(), stone.max_count()),
            ("minecraft:stone", 5, Some(64))
        );
        let sword = inventory.slot(PlayerSlot::Storage(14)).unwrap();
        let large = inventory.slot(PlayerSlot::Storage(4)).unwrap();
        assert_eq!((large.count(), large.max_count()), (99, Some(99)));
        assert_eq!(
            large.components().get("minecraft:max_stack_size"),
            Some(&Tag::Int(99))
        );
        assert_eq!(sword.id(), "minecraft:diamond_sword");
        assert_eq!(sword.max_count(), Some(1));
        assert_eq!(
            sword
                .components()
                .get("minecraft:custom_data")
                .and_then(|v| v.get("marker")),
            Some(&Tag::Int(7654))
        );
        assert_eq!(
            inventory.slot(PlayerSlot::Offhand).unwrap().id(),
            "minecraft:shield"
        );
        let book = inventory.recipe_book();
        assert!(book.open && book.filtering);
        assert_eq!(book.known.len(), 2);
        assert!(
            book.known
                .contains(&ResourceId::parse("minecraft:oak_planks").unwrap())
        );
        assert!(
            book.known
                .contains(&ResourceId::parse("minecraft:stick").unwrap())
        );
        assert_eq!(book.highlighted.len(), 1);
        assert!(
            book.highlighted
                .contains(&ResourceId::parse("minecraft:stick").unwrap())
        );

        let mut changed = inventory.clone();
        changed.select(4);
        changed.set_slot(
            PlayerSlot::Storage(3),
            ItemStack::new("minecraft:stone".into(), 8, 64, Tag::Compound(vec![])),
        );
        let sword = changed.slot(PlayerSlot::Storage(14)).cloned().unwrap();
        changed.set_slot(PlayerSlot::Storage(14), None);
        changed.set_slot(PlayerSlot::Storage(30), Some(sword));
        changed.set_slot(
            PlayerSlot::Armor(0),
            ItemStack::new(
                "minecraft:diamond_sword".into(),
                1,
                1,
                Tag::Compound(vec![]),
            ),
        );
        let shield = changed.slot(PlayerSlot::Offhand).cloned().unwrap();
        changed.set_slot(
            PlayerSlot::Offhand,
            ItemStack::new("minecraft:stone".into(), 7, 64, Tag::Compound(vec![])),
        );
        changed.set_slot(PlayerSlot::Storage(5), Some(shield));
        let mut book = changed.recipe_book().clone();
        let torch = ResourceId::parse("minecraft:torch").unwrap();
        book.known.insert(torch.clone());
        book.highlighted.insert(torch);
        book.open = false;
        changed.set_recipe_book(book);
        let draft = draft_player_inventory(&player, &changed).unwrap();
        let raw = temp.path().join("rust-player.nbt");
        fs::write(
            &raw,
            native_persistence::nbt::write(draft.document()).unwrap(),
        )
        .unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("InventorySaveOracle")
            .arg(&raw)
            .arg("check"));
    }
}
