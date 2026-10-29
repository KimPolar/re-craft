//! Read item-entity state from a 26.3 entity chunk without modifying its NBT.
//!
//! This is preparation for live entity loading. It does not by itself spawn,
//! tick, pick up, save or discard entities; callers must account for UUIDs,
//! chunk eviction, owners and unsupported entity behavior first.

use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use glam::DVec3;
use native_persistence::nbt::{NamedTag, Tag, TagType};
use native_resources::packs::ResourceId;

use crate::{inventory::ItemStack, player_data};

#[derive(Clone, Debug)]
pub struct SavedItemState {
    /// The source is retained for unsupported fields and eventual safe saving.
    pub source: Arc<Tag>,
    /// Java creates an identity when missing; callers cannot deduplicate it yet.
    pub uuid: Option<[i32; 4]>,
    pub position: DVec3,
    pub velocity: DVec3,
    pub on_ground: bool,
    pub no_gravity: bool,
    pub age: i16,
    pub pickup_delay: i16,
    pub health: i16,
    pub fire_ticks: i16,
    pub invulnerable: bool,
    pub invulnerable_time: i32,
    pub owner: Option<[i32; 4]>,
    pub thrower: Option<[i32; 4]>,
    pub stack: ItemStack,
}

/// Simulated state for a still-existing saved item. This is not sufficient to
/// commit a world: pickup, merges, entity creation and chunk migration must be
/// coordinated with the other affected data.
pub struct ItemSaveState<'a> {
    pub position: DVec3,
    pub velocity: DVec3,
    pub on_ground: bool,
    pub no_gravity: bool,
    pub age: i16,
    pub pickup_delay: i16,
    pub health: i16,
    pub fire_ticks: i16,
    pub invulnerable: bool,
    pub invulnerable_time: i32,
    pub stack: &'a ItemStack,
}

pub struct ItemDraft {
    source_nbt: Vec<u8>,
    document: NamedTag,
}

/// A new item document has no original entity; a caller must still coordinate
/// its creation, UUID, chunk membership and all effects in the world save.
pub struct CreatedItemDraft {
    document: NamedTag,
}

/// Base entity fields that cannot be inferred from item-specific state.
pub struct CreatedItemBase {
    pub rotation: [f32; 2],
    pub fall_distance: f64,
    pub air: i16,
    pub portal_cooldown: i32,
}

impl CreatedItemDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
}

fn validate_created_custom_data(value: &Tag, depth: usize) -> Result<()> {
    ensure!(depth <= 512, "item custom data exceeds supported nesting");
    match value {
        Tag::Compound(fields) => {
            let mut keys = std::collections::BTreeSet::new();
            for (name, value) in fields {
                ensure!(
                    keys.insert(name.0.clone()),
                    "duplicate item custom-data field"
                );
                validate_created_custom_data(value, depth + 1)?;
            }
        }
        Tag::List { values, .. } => {
            for value in values {
                validate_created_custom_data(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Encode a new ordinary item entity with a caller-supplied stable UUID.
/// The caller must supply the actual ownership and all simulated fields.
pub fn draft_created(
    uuid: [i32; 4],
    owner: Option<[i32; 4]>,
    thrower: Option<[i32; 4]>,
    base: CreatedItemBase,
    state: ItemSaveState<'_>,
) -> Result<CreatedItemDraft> {
    ensure!(
        state.position.is_finite()
            && state.position.x.abs() <= 30_000_512.0
            && state.position.z.abs() <= 30_000_512.0
            && state.position.y.abs() <= 20_000_000.0,
        "position would be clamped"
    );
    ensure!(
        state.velocity.is_finite() && state.velocity.abs().max_element() <= 10.0,
        "velocity would be discarded"
    );
    ensure!(
        state.invulnerable_time >= 0,
        "invalid invulnerability duration"
    );
    ensure!(
        base.rotation.iter().all(|value| value.is_finite())
            && base.fall_distance.is_finite()
            && base.fall_distance >= 0.0
            && base.portal_cooldown >= 0,
        "invalid base entity state"
    );
    ensure!(
        (1..=99).contains(&state.stack.count()),
        "item count outside codec range"
    );
    ResourceId::parse(state.stack.id()).context("invalid item identifier")?;
    ensure!(
        state.stack.max_count().is_some()
            && crate::equippable::saved_inventory_max(state.stack.id(), state.stack.components())
                == state.stack.max_count(),
        "item prototype cannot be reconstructed from saved components"
    );
    let Tag::Compound(components) = state.stack.components() else {
        bail!("invalid item components")
    };
    let mut keys = std::collections::BTreeSet::new();
    for (name, value) in components {
        ensure!(keys.insert(name.0.clone()), "duplicate item component");
        ensure!(
            (name.matches("minecraft:max_stack_size") && matches!(value, Tag::Int(1..=99)))
                || (name.matches("minecraft:damage") && matches!(value, Tag::Int(0..)))
                || (name.matches("minecraft:custom_data") && matches!(value, Tag::Compound(_))),
            "unsupported new item component"
        );
        if name.matches("minecraft:custom_data") {
            validate_created_custom_data(value, 0)?;
        }
    }
    let vector = |v: DVec3| Tag::List {
        element_type: TagType::Double,
        values: v.to_array().into_iter().map(Tag::Double).collect(),
    };
    let mut fields = vec![
        ("id".into(), Tag::String("minecraft:item".into())),
        ("UUID".into(), Tag::IntArray(uuid.to_vec())),
        ("Pos".into(), vector(state.position)),
        ("Motion".into(), vector(state.velocity)),
        (
            "Rotation".into(),
            Tag::List {
                element_type: TagType::Float,
                values: base.rotation.into_iter().map(Tag::Float).collect(),
            },
        ),
        ("fall_distance".into(), Tag::Double(base.fall_distance)),
        ("Air".into(), Tag::Short(base.air)),
        ("PortalCooldown".into(), Tag::Int(base.portal_cooldown)),
        ("OnGround".into(), Tag::Byte(i8::from(state.on_ground))),
        ("NoGravity".into(), Tag::Byte(i8::from(state.no_gravity))),
        ("Age".into(), Tag::Short(state.age)),
        ("PickupDelay".into(), Tag::Short(state.pickup_delay)),
        ("Health".into(), Tag::Short(state.health)),
        ("Fire".into(), Tag::Short(state.fire_ticks)),
        (
            "Invulnerable".into(),
            Tag::Byte(i8::from(state.invulnerable)),
        ),
        (
            "invulnerable_time".into(),
            Tag::Int(state.invulnerable_time),
        ),
        ("Item".into(), player_data::encode_stack(state.stack, None)),
    ];
    if let Some(owner) = owner {
        fields.push(("Owner".into(), Tag::IntArray(owner.to_vec())));
    }
    if let Some(thrower) = thrower {
        fields.push(("Thrower".into(), Tag::IntArray(thrower.to_vec())));
    }
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(fields),
    };
    // Re-read using the same strict path used for loaded entities.
    ensure!(
        read(&document.value)?.is_some(),
        "cannot decode new item entity"
    );
    native_persistence::nbt::write(&document).context("cannot encode new item entity")?;
    Ok(CreatedItemDraft { document })
}

impl ItemDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
    pub fn matches_source(&self, current: &NamedTag) -> Result<bool> {
        Ok(native_persistence::nbt::write(current)? == self.source_nbt)
    }
}

/// Update a supported item's fields while retaining its original stack and
/// unrelated NBT. Identity, ownership and component changes need separate
/// handling; an item removed or merged away cannot be represented here.
pub fn draft(original: &NamedTag, state: ItemSaveState<'_>) -> Result<ItemDraft> {
    let Tag::Compound(fields) = &original.value else {
        bail!("entity must be a compound")
    };
    for key in [
        "id",
        "UUID",
        "Pos",
        "Motion",
        "OnGround",
        "NoGravity",
        "Age",
        "PickupDelay",
        "Health",
        "Fire",
        "Invulnerable",
        "invulnerable_time",
        "Owner",
        "Thrower",
        "Item",
        "Passengers",
    ] {
        ensure!(
            fields.iter().filter(|(name, _)| name.matches(key)).count() <= 1,
            "duplicate entity {key}"
        );
    }
    ensure!(
        original.value.get("Passengers").is_none(),
        "passengers are not simulated"
    );
    let saved = read(&original.value)?.context("entity is not an item")?;
    ensure!(saved.uuid.is_some(), "item has no stable UUID");
    ensure!(
        saved.stack.id() == state.stack.id()
            && saved.stack.components() == state.stack.components(),
        "item identity or components changed"
    );
    ensure!(
        (1..=99).contains(&state.stack.count()),
        "item count outside codec range"
    );
    let item = original.value.get("Item").context("missing item stack")?;
    let Tag::Compound(item_fields) = item else {
        bail!("item stack must be a compound")
    };
    for key in ["id", "count", "components"] {
        ensure!(
            item_fields
                .iter()
                .filter(|(name, _)| name.matches(key))
                .count()
                <= 1,
            "duplicate item {key}"
        );
    }
    if let Some(count) = item.get("count") {
        ensure!(matches!(count, Tag::Int(1..=99)), "unsupported item count");
    }
    for key in ["Pos", "Motion"] {
        if let Some(value) = original.value.get(key) {
            ensure!(
                matches!(value, Tag::List { element_type: TagType::Double, values } if values.len() == 3 && values.iter().all(|v| matches!(v, Tag::Double(x) if x.is_finite()))),
                "unsupported {key}"
            );
        }
    }
    for key in ["OnGround", "NoGravity", "Invulnerable"] {
        if let Some(value) = original.value.get(key) {
            ensure!(matches!(value, Tag::Byte(0 | 1)), "unsupported {key}");
        }
    }
    for key in ["Age", "PickupDelay", "Health", "Fire"] {
        if let Some(value) = original.value.get(key) {
            ensure!(matches!(value, Tag::Short(_)), "unsupported {key}");
        }
    }
    if let Some(value) = original.value.get("invulnerable_time") {
        ensure!(
            matches!(value, Tag::Int(_)),
            "unsupported invulnerable_time"
        );
    }
    let valid_position = |v: DVec3| {
        v.is_finite()
            && v.x.abs() <= 30_000_512.0
            && v.z.abs() <= 30_000_512.0
            && v.y.abs() <= 20_000_000.0
    };
    let raw_vector = |key: &str| -> Option<DVec3> {
        let Tag::List { values, .. } = original.value.get(key)? else {
            return None;
        };
        let [Tag::Double(x), Tag::Double(y), Tag::Double(z)] = values.as_slice() else {
            return None;
        };
        Some(DVec3::new(*x, *y, *z))
    };
    ensure!(
        raw_vector("Pos").is_some_and(valid_position) && valid_position(state.position),
        "position would be clamped"
    );
    ensure!(
        raw_vector("Motion").is_none_or(|v| v.abs().max_element() <= 10.0)
            && state.velocity.is_finite()
            && state.velocity.abs().max_element() <= 10.0,
        "velocity would be discarded"
    );
    let source_nbt = native_persistence::nbt::write(original)?;
    let mut document = original.clone();
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    let vector = |v: DVec3| Tag::List {
        element_type: TagType::Double,
        values: v.to_array().into_iter().map(Tag::Double).collect(),
    };
    for (name, value) in [
        ("Pos", vector(state.position)),
        ("Motion", vector(state.velocity)),
        ("OnGround", Tag::Byte(i8::from(state.on_ground))),
        ("NoGravity", Tag::Byte(i8::from(state.no_gravity))),
        ("Age", Tag::Short(state.age)),
        ("PickupDelay", Tag::Short(state.pickup_delay)),
        ("Health", Tag::Short(state.health)),
        ("Fire", Tag::Short(state.fire_ticks)),
        ("Invulnerable", Tag::Byte(i8::from(state.invulnerable))),
        ("invulnerable_time", Tag::Int(state.invulnerable_time)),
    ] {
        if let Some((_, existing)) = fields.iter_mut().find(|(key, _)| key.matches(name)) {
            *existing = value;
        } else {
            fields.push((name.into(), value));
        }
    }
    let (_, Tag::Compound(item_fields)) = fields
        .iter_mut()
        .find(|(key, _)| key.matches("Item"))
        .unwrap()
    else {
        unreachable!()
    };
    if let Some((_, count)) = item_fields.iter_mut().find(|(key, _)| key.matches("count")) {
        *count = Tag::Int(state.stack.count() as i32);
    } else {
        item_fields.push(("count".into(), Tag::Int(state.stack.count() as i32)));
    }
    Ok(ItemDraft {
        source_nbt,
        document,
    })
}

fn uuid(value: &Tag) -> Result<[i32; 4]> {
    let values: Vec<i32> = match value {
        Tag::IntArray(values) => values.clone(),
        Tag::List {
            element_type: TagType::Int,
            values,
        } => values
            .iter()
            .map(|value| {
                if let Tag::Int(value) = value {
                    Ok(*value)
                } else {
                    bail!("UUID contains a non-integer")
                }
            })
            .collect::<Result<_>>()?,
        _ => bail!("UUID must be a sequence of four integers"),
    };
    values
        .try_into()
        .map_err(|_| anyhow::anyhow!("UUID must contain exactly four integers"))
}

pub(crate) fn optional_uuid(entity: &Tag, key: &str) -> Result<Option<[i32; 4]>> {
    entity
        .get(key)
        .map(uuid)
        .transpose()
        .with_context(|| format!("invalid {key} UUID"))
}

pub(crate) fn short(value: Option<&Tag>, default: i16) -> i16 {
    match value {
        Some(Tag::Byte(value)) => i16::from(*value),
        Some(Tag::Short(value)) => *value,
        Some(Tag::Int(value)) => *value as i16,
        Some(Tag::Long(value)) => *value as i16,
        Some(Tag::Float(value)) => player_data::java_floor(f64::from(*value)) as i16,
        Some(Tag::Double(value)) => player_data::java_floor(*value) as i16,
        _ => default,
    }
}

/// Returns `None` for non-item entities. A malformed item is reported rather
/// than converted to an empty stack or silently altered.
pub fn read(entity: &Tag) -> Result<Option<SavedItemState>> {
    ensure!(
        matches!(entity, Tag::Compound(_)),
        "entity must be a compound"
    );
    let id = match entity.get("id") {
        Some(Tag::String(value)) => {
            ResourceId::parse(&String::from_utf16(&value.0).context("invalid entity id UTF-16")?)?
        }
        _ => bail!("entity is missing a valid id"),
    };
    if id.namespace != "minecraft" || id.path != "item" {
        return Ok(None);
    }
    let named = NamedTag {
        name: "".into(),
        value: entity.clone(),
    };
    let position =
        player_data::read_saved_position(&named)?.context("saved item is missing its position")?;
    let motion = player_data::read_saved_motion(&named)?;
    let no_gravity = entity
        .get("NoGravity")
        .and_then(player_data::numeric_byte)
        .is_some_and(|value| value != 0);
    let stack = player_data::read_stack(entity.get("Item").context("saved item has no stack")?)?;
    Ok(Some(SavedItemState {
        source: Arc::new(named.value),
        uuid: optional_uuid(entity, "UUID")?,
        position,
        velocity: motion.velocity,
        on_ground: motion.on_ground,
        no_gravity,
        age: short(entity.get("Age"), 0),
        pickup_delay: short(entity.get("PickupDelay"), 0),
        health: short(entity.get("Health"), 5),
        fire_ticks: short(entity.get("Fire"), 0),
        invulnerable: entity
            .get("Invulnerable")
            .and_then(player_data::numeric_byte)
            .is_some_and(|value| value != 0),
        invulnerable_time: entity
            .get("invulnerable_time")
            .and_then(player_data::numeric_int)
            .unwrap_or(0),
        owner: optional_uuid(entity, "Owner")?,
        thrower: optional_uuid(entity, "Thrower")?,
        stack,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::JavaString;

    #[test]
    fn created_item_rejects_unresolved_or_invalid_state() {
        let ordinary =
            ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        let unknown =
            ItemStack::unresolved("example:unknown".into(), 1, Tag::Compound(vec![])).unwrap();
        let unsupported = ItemStack::new(
            "minecraft:stone".into(),
            1,
            64,
            Tag::Compound(vec![("minecraft:custom_name".into(), Tag::Int(5))]),
        )
        .unwrap();
        let duplicate_data = ItemStack::new(
            "minecraft:stone".into(),
            1,
            64,
            Tag::Compound(vec![(
                "minecraft:custom_data".into(),
                Tag::Compound(vec![("x".into(), Tag::Int(1)), ("x".into(), Tag::Int(2))]),
            )]),
        )
        .unwrap();
        let base = || CreatedItemBase {
            rotation: [0.0, 0.0],
            fall_distance: 0.0,
            air: 300,
            portal_cooldown: 0,
        };
        fn state(stack: &ItemStack) -> ItemSaveState<'_> {
            ItemSaveState {
                position: DVec3::new(1.0, 2.0, 3.0),
                velocity: DVec3::ZERO,
                on_ground: false,
                no_gravity: false,
                age: 0,
                pickup_delay: 10,
                health: 5,
                fire_ticks: 0,
                invulnerable: false,
                invulnerable_time: 0,
                stack,
            }
        }
        assert!(draft_created([1, 2, 3, 4], None, None, base(), state(&ordinary)).is_ok());
        assert!(draft_created([1, 2, 3, 4], None, None, base(), state(&unknown)).is_err());
        assert!(draft_created([1, 2, 3, 4], None, None, base(), state(&unsupported)).is_err());
        assert!(draft_created([1, 2, 3, 4], None, None, base(), state(&duplicate_data)).is_err());
        let mut invalid = state(&ordinary);
        invalid.velocity.x = 11.0;
        assert!(draft_created([1, 2, 3, 4], None, None, base(), invalid).is_err());
        let mut invalid_base = base();
        invalid_base.rotation[0] = f32::NAN;
        assert!(draft_created([1, 2, 3, 4], None, None, invalid_base, state(&ordinary)).is_err());
    }

    fn vector(v: [f64; 3]) -> Tag {
        Tag::List {
            element_type: TagType::Double,
            values: v.into_iter().map(Tag::Double).collect(),
        }
    }
    fn item() -> Tag {
        Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            ("Pos".into(), vector([1.25, 64.0, -2.5])),
            ("Motion".into(), vector([0.125, -0.25, 15.0])),
            ("Age".into(), Tag::Short(i16::MIN)),
            ("PickupDelay".into(), Tag::Short(i16::MAX)),
            ("Owner".into(), Tag::IntArray(vec![9, 8, 7, 6])),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:diamond_sword".into())),
                    ("count".into(), Tag::Int(1)),
                    (
                        "components".into(),
                        Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(17))]),
                    ),
                ]),
            ),
            (
                JavaString::from("unknown_plugin_field"),
                Tag::Long(987654321),
            ),
        ])
    }

    #[test]
    fn saved_item_keeps_original_data_and_special_lifetimes() {
        let input = item();
        let state = read(&input).unwrap().unwrap();
        assert_eq!(*state.source, input);
        assert_eq!(state.uuid, Some([1, 2, 3, 4]));
        assert_eq!(state.owner, Some([9, 8, 7, 6]));
        assert_eq!(state.thrower, None);
        assert_eq!(state.age, i16::MIN);
        assert_eq!(state.pickup_delay, i16::MAX);
        assert_eq!(state.health, 5);
        assert_eq!(state.position, DVec3::new(1.25, 64.0, -2.5));
        assert_eq!(state.velocity, DVec3::new(0.125, -0.25, 0.0));
        assert_eq!(state.stack.id(), "minecraft:diamond_sword");
        assert_eq!(
            state.stack.components().get("minecraft:damage"),
            Some(&Tag::Int(17))
        );
    }

    #[test]
    fn item_draft_preserves_original_data_and_rejects_unsafe_changes() {
        let mut entity = item();
        let Tag::Compound(fields) = &mut entity else {
            unreachable!()
        };
        fields
            .iter_mut()
            .find(|(key, _)| key.matches("Motion"))
            .unwrap()
            .1 = vector([0.125, -0.25, 0.5]);
        let (_, Tag::Compound(stack)) = fields
            .iter_mut()
            .find(|(key, _)| key.matches("Item"))
            .unwrap()
        else {
            unreachable!()
        };
        stack[0].1 = Tag::String("minecraft:stone".into());
        stack.push(("unknown_stack_field".into(), Tag::Long(55)));
        let original = NamedTag {
            name: "".into(),
            value: entity,
        };
        let saved = read(&original.value).unwrap().unwrap();
        let stack = saved.stack.with_count(3).unwrap();
        let state = || ItemSaveState {
            position: DVec3::new(2.0, 63.0, -2.0),
            velocity: DVec3::ZERO,
            on_ground: true,
            no_gravity: false,
            age: 42,
            pickup_delay: 9,
            health: 4,
            fire_ticks: 12,
            invulnerable: true,
            invulnerable_time: 3,
            stack: &stack,
        };
        let result = draft(&original, state()).unwrap();
        assert!(result.matches_source(&original).unwrap());
        assert_eq!(
            result.document().value.get("unknown_plugin_field"),
            Some(&Tag::Long(987654321))
        );
        let output_stack = result.document().value.get("Item").unwrap();
        assert_eq!(
            output_stack.get("unknown_stack_field"),
            Some(&Tag::Long(55))
        );
        assert_eq!(output_stack.get("count"), Some(&Tag::Int(3)));
        let decoded = read(&result.document().value).unwrap().unwrap();
        assert_eq!(decoded.age, 42);
        assert_eq!(decoded.stack.count(), 3);
        assert_eq!(decoded.owner, saved.owner);
        let mut changed = original.clone();
        let Tag::Compound(fields) = &mut changed.value else {
            unreachable!()
        };
        fields.push(("Age".into(), Tag::Short(2)));
        assert!(!result.matches_source(&changed).unwrap());
        assert!(draft(&changed, state()).is_err());
        let mut invalid = state();
        invalid.velocity.x = 11.0;
        assert!(draft(&original, invalid).is_err());
    }

    #[test]
    fn saved_damage_fields_follow_java_numeric_conversions() {
        let mut input = item();
        if let Tag::Compound(fields) = &mut input {
            fields.extend([
                ("Health".into(), Tag::Short(3)),
                ("Fire".into(), Tag::Double(-1.25)),
                ("Invulnerable".into(), Tag::Int(257)),
                ("invulnerable_time".into(), Tag::Double(7.75)),
            ]);
        }
        let saved = read(&input).unwrap().unwrap();
        assert_eq!(saved.health, 3);
        assert_eq!(saved.fire_ticks, -2);
        assert!(saved.invulnerable);
        assert_eq!(saved.invulnerable_time, 7);
    }

    #[test]
    fn malformed_identity_or_stack_is_never_silently_replaced() {
        let mut bad = item();
        if let Tag::Compound(values) = &mut bad {
            values.push(("UUID".into(), Tag::IntArray(vec![1, 2])));
        }
        assert!(read(&bad).is_err());
        let other = Tag::Compound(vec![("id".into(), Tag::String("minecraft:zombie".into()))]);
        assert!(read(&other).unwrap().is_none());
        assert!(
            read(&Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:item".into())
            )]))
            .is_err()
        );
        assert_eq!(short(Some(&Tag::Double(-1.25)), 0), -2);
        assert_eq!(short(Some(&Tag::Int(65535)), 0), -1);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn saved_item_fields_match_java_item_entity() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
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
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SavedItemOracle.java");
        std::fs::write(&source, include_str!("../tests/java/SavedItemOracle.java")).unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "saved item Java oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("SavedItemOracle")
            .arg(temp.path()));
        let expected: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(temp.path().join("expected.json")).unwrap())
                .unwrap();
        assert_eq!(expected.len(), 7);
        for (index, row) in expected.iter().enumerate() {
            let file = temp.path().join(format!("item-{index}.nbt"));
            let bytes = std::fs::read(&file).unwrap();
            let original = native_persistence::nbt::read(&bytes).unwrap();
            let saved = read(&original.value).unwrap().unwrap();
            assert_eq!(
                i64::from(saved.age),
                row["age"].as_i64().unwrap(),
                "case {index}"
            );
            assert_eq!(
                i64::from(saved.pickup_delay),
                row["delay"].as_i64().unwrap(),
                "case {index}"
            );
            assert_eq!(
                i64::from(saved.health),
                row["health"].as_i64().unwrap(),
                "case {index}"
            );
            assert_eq!(
                i64::from(saved.fire_ticks),
                row["fire"].as_i64().unwrap(),
                "case {index}"
            );
            assert_eq!(
                saved.invulnerable,
                row["invulnerable"].as_bool().unwrap(),
                "case {index}"
            );
            assert_eq!(
                i64::from(saved.invulnerable_time),
                row["invulnerable_time"].as_i64().unwrap(),
                "case {index}"
            );
            assert_eq!(saved.stack.id(), row["item"].as_str().unwrap());
            assert_eq!(
                saved
                    .stack
                    .components()
                    .get("minecraft:damage")
                    .and_then(Tag::as_java_int),
                Some(row["damage"].as_i64().unwrap() as i32)
            );
            assert_eq!(saved.owner, Some([9, 8, 7, 6]));
            assert_eq!(row["owner_matches"].as_bool(), Some(true));
            assert_eq!(*saved.source, original.value);
            assert_eq!(std::fs::read(file).unwrap(), bytes, "source file changed");
        }
        let mut original =
            native_persistence::nbt::read(&std::fs::read(temp.path().join("item-0.nbt")).unwrap())
                .unwrap();
        let Tag::Compound(fields) = &mut original.value else {
            unreachable!()
        };
        for (key, value) in [
            ("Motion", vector([0.125, -0.25, 0.5])),
            ("Age", Tag::Short(0)),
            ("PickupDelay", Tag::Short(0)),
            ("Health", Tag::Short(5)),
            ("Fire", Tag::Short(0)),
            ("Invulnerable", Tag::Byte(0)),
            ("invulnerable_time", Tag::Int(0)),
        ] {
            fields
                .iter_mut()
                .find(|(name, _)| name.matches(key))
                .unwrap()
                .1 = value;
        }
        let saved = read(&original.value).unwrap().unwrap();
        let draft = draft(
            &original,
            ItemSaveState {
                position: DVec3::new(2.0, 63.0, -2.0),
                velocity: DVec3::ZERO,
                on_ground: true,
                no_gravity: false,
                age: 42,
                pickup_delay: 9,
                health: 4,
                fire_ticks: 12,
                invulnerable: true,
                invulnerable_time: 3,
                stack: &saved.stack,
            },
        )
        .unwrap();
        let draft_path = temp.path().join("item-draft.nbt");
        std::fs::write(
            &draft_path,
            native_persistence::nbt::write(draft.document()).unwrap(),
        )
        .unwrap();
        let created_stack = ItemStack::new(
            "minecraft:stone".into(),
            3,
            99,
            Tag::Compound(vec![("minecraft:max_stack_size".into(), Tag::Int(99))]),
        )
        .unwrap();
        let created = draft_created(
            [1, 2, 3, 4],
            Some([9, 8, 7, 6]),
            Some([5, 6, 7, 8]),
            CreatedItemBase {
                rotation: [30.0, -15.0],
                fall_distance: 2.5,
                air: 280,
                portal_cooldown: 7,
            },
            ItemSaveState {
                position: DVec3::new(2.0, 63.0, -2.0),
                velocity: DVec3::new(0.25, -0.5, 0.0),
                on_ground: false,
                no_gravity: false,
                age: 42,
                pickup_delay: 9,
                health: 4,
                fire_ticks: 12,
                invulnerable: true,
                invulnerable_time: 3,
                stack: &created_stack,
            },
        )
        .unwrap();
        let created_path = temp.path().join("created-item.nbt");
        std::fs::write(
            &created_path,
            native_persistence::nbt::write(created.document()).unwrap(),
        )
        .unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("SavedItemOracle")
            .arg(temp.path())
            .arg(draft_path)
            .arg(created_path));
        let resistance: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(temp.path().join("resistance.json")).unwrap())
                .unwrap();
        for (index, row) in resistance.iter().enumerate() {
            let bytes = std::fs::read(temp.path().join(format!("resistance-{index}.nbt"))).unwrap();
            let component = native_persistence::nbt::read(&bytes).unwrap().value;
            let stack = crate::inventory::ItemStack::new(
                "minecraft:dirt".into(),
                1,
                64,
                Tag::Compound(vec![("minecraft:damage_resistant".into(), component)]),
            )
            .unwrap();
            let actual = crate::items::damage_resistant(
                &stack,
                crate::items::DamageResistancePrototype::Unknown,
                "minecraft:lava",
                |tag, damage| Ok(tag == "minecraft:is_fire" && damage == "minecraft:lava"),
                |id| Ok(matches!(id, "minecraft:lava" | "minecraft:fall")),
            );
            assert_eq!(
                actual.is_ok(),
                row["valid"].as_bool().unwrap(),
                "codec case {index}: {actual:?}"
            );
            if let Ok(value) = actual {
                assert_eq!(value, row["lava"].as_bool(), "codec case {index}");
            }
        }
    }
}
