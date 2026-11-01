//! In-memory item drops for the currently supported basic block loot.
//!
//! Fluid movement, item-specific effects, entity saving and the
//! world's exact random stream are not implemented yet.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::{Arc, LazyLock},
};

use glam::{DVec3, IVec3};
use native_core::random::LegacyRandom;
use native_persistence::nbt::Tag;
use native_resources::loot::{BasicTool, BlockLootTables, Drop};
use native_resources::transforms::DropStrategy;
use native_world::{collision::Aabb, movement::BoundingBox};

use crate::{
    GameMode, PlayerPose, PlayerState,
    inventory::{ItemStack, PlayerInventory, ordinary_item_default_max},
    mining::{self, Material},
    movement,
};

fn prepare_component_copies(drops: &[Drop], components: &Tag) -> anyhow::Result<Vec<Tag>> {
    use anyhow::{Context, ensure};
    use native_resources::packs::ResourceId;
    let Tag::Compound(source) = components else {
        anyhow::bail!("block entity components are not a compound")
    };
    let mut prepared = Vec::with_capacity(drops.len());
    for drop in drops {
        let mut selected = Vec::new();
        let mut seen = BTreeSet::new();
        for (key, value) in source {
            let name = String::from_utf16(&key.0).context("invalid component identifier")?;
            let id = ResourceId::parse(&name)?;
            ensure!(seen.insert(id.clone()), "duplicate block entity component");
            if (drop.copy_block_entity_custom_name
                && id.namespace == "minecraft"
                && id.path == "custom_name")
                || drop
                    .copy_block_entity_components
                    .iter()
                    .any(|filter| filter.accepts(&id))
            {
                selected.push((key.clone(), value.clone()));
            }
        }
        let components = Tag::Compound(selected);
        let item = format!("{}:{}", drop.item.namespace, drop.item.path);
        ensure!(
            crate::inventory::saved_item_max(&item, &components).is_some(),
            "unsupported dropped item components"
        );
        prepared.push(components);
    }
    Ok(prepared)
}

#[derive(Clone, Debug)]
pub struct DroppedItem {
    /// Session-local entity identifier used by item model properties.
    pub entity_id: i32,
    /// Java item lifetime (distinct from Entity.tickCount, used below as age).
    pub item_age: i32,
    /// Absolute position, independent of streaming's relative coordinate origin.
    pub position: DVec3,
    /// Position at the preceding tick, for render interpolation.
    pub previous_position: DVec3,
    pub stack: ItemStack,
    pub age: u32,
    /// A stable per-entity phase. The world's exact entity RNG is not yet restored.
    pub bob_offset: f32,
}

/// Stack-level portion of Java's item-entity merge rule. Entity lifetime,
/// ownership, position and tick scheduling are checked separately.
pub fn stacks_mergeable(first: &ItemStack, second: &ItemStack) -> bool {
    first.same_item_and_components(second)
        && second.max_count().is_some_and(|max| {
            first
                .count()
                .checked_add(second.count())
                .is_some_and(|sum| sum <= max)
        })
}

/// Restitution of 26.3 vanilla blocks that make nonliving entities bounce.
pub(crate) fn block_bounce_restitution(name: &str) -> f32 {
    match name {
        "minecraft:slime_block" => 1.0,
        "minecraft:shelf_mushroom" => 0.75,
        name if matches!(
            name.strip_prefix("minecraft:"),
            Some(
                "white_bed"
                    | "orange_bed"
                    | "magenta_bed"
                    | "light_blue_bed"
                    | "yellow_bed"
                    | "lime_bed"
                    | "pink_bed"
                    | "gray_bed"
                    | "light_gray_bed"
                    | "cyan_bed"
                    | "purple_bed"
                    | "blue_bed"
                    | "brown_bed"
                    | "green_bed"
                    | "red_bed"
                    | "black_bed"
            )
        ) =>
        {
            0.75
        }
        _ => 0.0,
    }
}

pub(crate) fn vertical_restitution(
    velocity: f64,
    actual_y: f64,
    restitution: f64,
    gravity: f64,
) -> f64 {
    if velocity >= -gravity || restitution <= 0.0 {
        return 0.0;
    }
    collision_restitution(velocity, actual_y, restitution, gravity)
}

pub(crate) fn collision_restitution(
    velocity: f64,
    actual_y: f64,
    restitution: f64,
    gravity: f64,
) -> f64 {
    if restitution <= 0.0 {
        return 0.0;
    }
    let ratio = actual_y / velocity;
    let air_drag = f64::from(0.98_f32);
    let drag = 1.0 + ratio * (air_drag - 1.0);
    (ratio * gravity - velocity) * drag * restitution
}

/// The prototype must be resolved from the installed item registry before a
/// missing component patch can be interpreted as non-resistant.
pub enum DamageResistancePrototype<'a> {
    Unknown,
    Absent,
    Component(&'a Tag),
}

/// Check the saved holder-set shape without assuming that referenced world
/// damage types or tags exist; the active registry checks those on use.
pub(crate) fn valid_damage_resistance_component(component: &Tag) -> bool {
    let Tag::Compound(fields) = component else {
        return false;
    };
    let mut found = fields.iter().filter(|(name, _)| name.matches("types"));
    let Some((_, types)) = found.next() else {
        return false;
    };
    if found.next().is_some() {
        return false;
    }
    let valid_id = |text: &native_persistence::nbt::JavaString, tag: bool| {
        let Ok(text) = String::from_utf16(&text.0) else {
            return false;
        };
        let text = if tag {
            text.strip_prefix('#').unwrap_or(&text)
        } else {
            &text
        };
        native_resources::packs::ResourceId::parse(text).is_ok()
    };
    match types {
        Tag::String(text) => valid_id(text, true),
        Tag::List {
            element_type: native_persistence::nbt::TagType::String,
            values,
        } => values
            .iter()
            .all(|value| matches!(value, Tag::String(text) if valid_id(text, false))),
        Tag::List {
            element_type: native_persistence::nbt::TagType::End,
            values,
        } => values.is_empty(),
        _ => false,
    }
}

/// Resolve an item's damage-resistance component against the active world's
/// damage-type tags. `None` means the item prototype has not been resolved.
pub fn damage_resistant(
    stack: &ItemStack,
    prototype: DamageResistancePrototype<'_>,
    damage_type: &str,
    mut tag_contains: impl FnMut(&str, &str) -> anyhow::Result<bool>,
    mut type_exists: impl FnMut(&str) -> anyhow::Result<bool>,
) -> anyhow::Result<Option<bool>> {
    use anyhow::{Context, ensure};
    use native_resources::packs::ResourceId;
    let damage = ResourceId::parse(damage_type)?;
    let canonical = format!("{}:{}", damage.namespace, damage.path);
    let Tag::Compound(entries) = stack.components() else {
        anyhow::bail!("invalid item components")
    };
    let mut added = None;
    let mut removed = None;
    for (key, value) in entries {
        let key = String::from_utf16(&key.0).context("invalid component id UTF-16")?;
        let (is_removed, name) = key
            .strip_prefix('!')
            .map_or((false, key.as_str()), |name| (true, name));
        let id = ResourceId::parse(name)?;
        if id.namespace == "minecraft" && id.path == "damage_resistant" {
            let slot = if is_removed { &mut removed } else { &mut added };
            ensure!(
                slot.replace(value).is_none(),
                "duplicate damage resistance component"
            );
        }
    }
    ensure!(
        added.is_none() || removed.is_none(),
        "conflicting damage resistance component patch"
    );
    if removed.is_some() {
        ensure!(
            matches!(removed, Some(Tag::Compound(fields)) if fields.is_empty()),
            "invalid removed damage resistance component"
        );
        return Ok(Some(false));
    }
    let component = if let Some(value) = added {
        value
    } else {
        match prototype {
            DamageResistancePrototype::Unknown => return Ok(None),
            DamageResistancePrototype::Absent => return Ok(Some(false)),
            DamageResistancePrototype::Component(value) => value,
        }
    };
    ensure!(
        valid_damage_resistance_component(component),
        "invalid damage resistance component"
    );
    let types = component
        .get("types")
        .context("damage resistance component is missing types")?;
    match types {
        Tag::String(value) => {
            let text = String::from_utf16(&value.0).context("invalid damage type tag UTF-16")?;
            if let Some(name) = text.strip_prefix('#') {
                let id = ResourceId::parse(name)?;
                Ok(Some(tag_contains(
                    &format!("{}:{}", id.namespace, id.path),
                    &canonical,
                )?))
            } else {
                let id = ResourceId::parse(&text)?;
                let id_text = format!("{}:{}", id.namespace, id.path);
                ensure!(type_exists(&id_text)?, "unregistered damage type {id_text}");
                Ok(Some(id == damage))
            }
        }
        Tag::List {
            element_type: native_persistence::nbt::TagType::String,
            values,
        } => {
            let mut matched = false;
            for value in values {
                let Tag::String(value) = value else {
                    anyhow::bail!("invalid damage type list")
                };
                let name = String::from_utf16(&value.0).context("invalid damage type UTF-16")?;
                let id = ResourceId::parse(&name)?;
                let id_text = format!("{}:{}", id.namespace, id.path);
                ensure!(type_exists(&id_text)?, "unregistered damage type {id_text}");
                matched |= id == damage;
            }
            Ok(Some(matched))
        }
        Tag::List {
            element_type: native_persistence::nbt::TagType::End,
            values,
        } if values.is_empty() => Ok(Some(false)),
        _ => anyhow::bail!("invalid damage resistance types"),
    }
}

/// Resolve verified vanilla defaults and the active world's damage types.
pub fn damage_resistant_in_world(
    stack: &ItemStack,
    items: &native_resources::item_registry::ItemRegistry,
    damage_types: &native_resources::enchantments::OxygenEnchantments,
    damage_type: &str,
) -> anyhow::Result<Option<bool>> {
    use native_resources::item_registry::DamageResistancePrototype as Builtin;
    static FIRE: LazyLock<Tag> = LazyLock::new(|| {
        Tag::Compound(vec![(
            "types".into(),
            Tag::String("#minecraft:is_fire".into()),
        )])
    });
    static EXPLOSION: LazyLock<Tag> = LazyLock::new(|| {
        Tag::Compound(vec![(
            "types".into(),
            Tag::String("#minecraft:is_explosion".into()),
        )])
    });
    let prototype = match items.damage_resistance_prototype(stack.id()) {
        Builtin::FireTag => DamageResistancePrototype::Component(&FIRE),
        Builtin::ExplosionTag => DamageResistancePrototype::Component(&EXPLOSION),
        Builtin::Absent => DamageResistancePrototype::Absent,
        Builtin::Unknown => DamageResistancePrototype::Unknown,
    };
    damage_resistant(
        stack,
        prototype,
        damage_type,
        |tag, damage| damage_types.damage_type_has_tag(tag, damage),
        |damage| damage_types.has_damage_type(damage),
    )
}

#[derive(Clone)]
struct ItemEntity {
    item: DroppedItem,
    velocity: DVec3,
    on_ground: bool,
    ground_without_support: bool,
    pickup_delay: i32,
    lifetime: i32,
    no_gravity: bool,
    owner: Option<[i32; 4]>,
    _saved_source: Option<Arc<Tag>>,
    created: Option<CreatedItemIdentity>,
    id: u32,
    section_sequence: u64,
    random: LegacyRandom,
    _damage_state: ItemDamageState,
}

#[derive(Clone, Debug, PartialEq)]
struct CreatedItemIdentity {
    uuid: Option<[i32; 4]>,
    origin: DVec3,
    thrower: CreatedThrower,
    yaw: f32,
    fall_distance: f64,
    /// Position and path for which every tick's base-entity state was checked.
    verified_position: DVec3,
    visited: BTreeSet<native_world::loaded::ChunkPos>,
    base_valid: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreatedThrower {
    Known(Option<[i32; 4]>),
    Unknown,
}

/// Whether Java attributes this drop to the player when creating the entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayerDropOrigin {
    Direct,
    ContainerReturn,
}

impl CreatedItemIdentity {
    fn new(origin: DVec3, thrower: CreatedThrower, yaw: f32, uuid: Option<[i32; 4]>) -> Self {
        let origin_chunk = created_chunk(origin);
        Self {
            uuid,
            origin,
            thrower,
            yaw,
            fall_distance: 0.0,
            verified_position: origin,
            visited: origin_chunk.into_iter().collect(),
            base_valid: origin_chunk.is_some(),
        }
    }
}

fn created_chunk(position: DVec3) -> Option<native_world::loaded::ChunkPos> {
    if !position.is_finite() || position.x.abs() > 30_000_512.0 || position.z.abs() > 30_000_512.0 {
        return None;
    }
    Some(native_world::loaded::ChunkPos {
        x: (position.x.floor() as i32).div_euclid(16),
        z: (position.z.floor() as i32).div_euclid(16),
    })
}

impl CreatedItemIdentity {
    fn verify_movement(&mut self, start: DVec3, end: DVec3, safe: bool) {
        self.base_valid &= safe && self.verified_position == start;
        let Some(a) = created_chunk(start) else {
            self.base_valid = false;
            return;
        };
        let Some(b) = created_chunk(end) else {
            self.base_valid = false;
            return;
        };
        let x0 = a.x.min(b.x);
        let x1 = a.x.max(b.x);
        let z0 = a.z.min(b.z);
        let z1 = a.z.max(b.z);
        if (i64::from(x1) - i64::from(x0) + 1) * (i64::from(z1) - i64::from(z0) + 1) > 64 {
            self.base_valid = false;
            return;
        }
        for x in x0..=x1 {
            for z in z0..=z1 {
                self.visited.insert(native_world::loaded::ChunkPos { x, z });
            }
        }
        self.verified_position = end;
    }
}

pub(crate) fn uuid_v4(mut bytes: [u8; 16]) -> [i32; 4] {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    std::array::from_fn(|i| i32::from_be_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()))
}

#[derive(Clone)]
struct RemovedSavedItem {
    entity: ItemEntity,
}

/// How a newly created item stopped existing in the in-memory simulation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreatedRemovalCause {
    /// A player collected the remainder; player inventory must be saved too.
    Pickup,
    /// The item reached its expiration age.
    Expired,
}

/// A verified inventory change or successful Java pickup for a created item.
/// Java awards a statistic only on success, using the pre-transfer count.
#[derive(Clone, Debug, PartialEq)]
pub struct CreatedItemPickup {
    pub uuid: [i32; 4],
    pub original_stack: ItemStack,
    pub transferred: u32,
    /// Creative mode may discard a remainder when the inventory is full.
    pub discarded: u32,
    /// Whether Java's `Inventory.add` reports success and triggers pickup effects.
    pub pickup_succeeded: bool,
}

struct CreatedPickupRecord {
    effect: CreatedItemPickup,
    chunks: BTreeSet<native_world::loaded::ChunkPos>,
}

struct RemovedCreatedItem {
    uuid: [i32; 4],
    chunks: BTreeSet<native_world::loaded::ChunkPos>,
    cause: CreatedRemovalCause,
}

struct SavedMergeParticipant {
    source: Arc<Tag>,
    position: DVec3,
}

struct SavedItemMerge {
    first: SavedMergeParticipant,
    second: SavedMergeParticipant,
}

#[derive(Default)]
struct MergeChanges {
    saved: Vec<SavedItemMerge>,
    unresolved: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct ItemDamageState {
    health: i32,
    fire_ticks: i32,
    invulnerable: bool,
    invulnerable_time: i32,
}

type ItemDamageRules = (
    Arc<native_resources::item_registry::ItemRegistry>,
    Arc<native_resources::enchantments::OxygenEnchantments>,
);

fn apply_lava_contact(entity: &mut ItemEntity, rules: &ItemDamageRules) -> anyhow::Result<bool> {
    let (items, tags) = rules;
    let Some(fire_immune) =
        damage_resistant_in_world(&entity.item.stack, items, tags, "minecraft:in_fire")?
    else {
        return Ok(false);
    };
    if fire_immune {
        return Ok(false);
    }
    let Some(resists_lava) =
        damage_resistant_in_world(&entity.item.stack, items, tags, "minecraft:lava")?
    else {
        return Ok(false);
    };
    let bypass = if entity._damage_state.invulnerable || entity._damage_state.invulnerable_time > 0
    {
        tags.damage_type_has_tag("minecraft:bypasses_invulnerability", "minecraft:lava")?
    } else {
        false
    };
    entity._damage_state.fire_ticks = entity._damage_state.fire_ticks.max(300);
    if !resists_lava
        && (!(entity._damage_state.invulnerable || entity._damage_state.invulnerable_time > 0)
            || bypass)
    {
        entity._damage_state.health = ((entity._damage_state.health as f32) - 4.0) as i32;
        return Ok(entity._damage_state.health <= 0);
    }
    Ok(false)
}

fn apply_fire_tick(
    entity: &mut ItemEntity,
    rules: &ItemDamageRules,
    in_lava: bool,
) -> anyhow::Result<bool> {
    if entity._damage_state.fire_ticks <= 0 {
        return Ok(false);
    }
    let (items, tags) = rules;
    let Some(fire_immune) =
        damage_resistant_in_world(&entity.item.stack, items, tags, "minecraft:in_fire")?
    else {
        return Ok(false);
    };
    if fire_immune {
        entity._damage_state.fire_ticks = 0;
        return Ok(false);
    }
    let mut destroyed = false;
    if entity._damage_state.fire_ticks % 20 == 0 && !in_lava {
        let Some(resists) =
            damage_resistant_in_world(&entity.item.stack, items, tags, "minecraft:on_fire")?
        else {
            return Ok(false);
        };
        let immune =
            entity._damage_state.invulnerable || entity._damage_state.invulnerable_time > 0;
        let bypass = if immune {
            tags.damage_type_has_tag("minecraft:bypasses_invulnerability", "minecraft:on_fire")?
        } else {
            false
        };
        if !resists && (!immune || bypass) {
            entity._damage_state.health = ((entity._damage_state.health as f32) - 1.0) as i32;
            destroyed = entity._damage_state.health <= 0;
        }
    }
    entity._damage_state.fire_ticks -= 1;
    Ok(destroyed)
}

impl Default for ItemDamageState {
    fn default() -> Self {
        Self {
            health: 5,
            fire_ticks: 0,
            invulnerable: false,
            invulnerable_time: 0,
        }
    }
}

pub(crate) struct ItemEntities {
    entities: Vec<ItemEntity>,
    // Retain actual removals separately from chunks that failed to load.
    removed_saved: Vec<RemovedSavedItem>,
    removed_created: Vec<RemovedCreatedItem>,
    created_pickups: Vec<CreatedPickupRecord>,
    pickup_sounds: VecDeque<DVec3>,
    extinguish_sounds: VecDeque<crate::ExtinguishSound>,
    removed_created_chunks: BTreeSet<native_world::loaded::ChunkPos>,
    invalid_removed_created_position: bool,
    saved_merges: Vec<SavedItemMerge>,
    uncoordinated_saved_merge: bool,
    // Unknown rain can affect a later merge, pickup, or removal, so retain it
    // until the session is restarted instead of accepting partial save drafts.
    unresolved_rain_effect: bool,
    random: LegacyRandom,
    entity_seeder: Option<native_core::random::XoroshiroRandom>,
    entity_seed_checked: bool,
    next_id: u32,
    next_section_sequence: u64,
    player_uuid: Option<[i32; 4]>,
    active_chunks: Option<BTreeSet<native_world::loaded::ChunkPos>>,
    damage_rules: Option<ItemDamageRules>,
}

impl Default for ItemEntities {
    fn default() -> Self {
        Self {
            entities: Vec::new(),
            removed_saved: Vec::new(),
            removed_created: Vec::new(),
            created_pickups: Vec::new(),
            pickup_sounds: VecDeque::new(),
            extinguish_sounds: VecDeque::new(),
            removed_created_chunks: BTreeSet::new(),
            invalid_removed_created_position: false,
            saved_merges: Vec::new(),
            uncoordinated_saved_merge: false,
            unresolved_rain_effect: false,
            random: LegacyRandom::new(0),
            entity_seeder: None,
            entity_seed_checked: false,
            next_id: 0,
            next_section_sequence: 0,
            player_uuid: None,
            active_chunks: None,
            damage_rules: None,
        }
    }
}

impl ItemEntities {
    fn ensure_rain_resolved(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.unresolved_rain_effect,
            "item rain effects could not be resolved"
        );
        Ok(())
    }
    /// Conservative closure of all known item sources, destinations, movement
    /// paths and merge/removal effects. Other entity kinds are not covered.
    pub(crate) fn affected_item_chunks(
        &self,
        saved_uuids: &std::collections::BTreeMap<[i32; 4], native_world::loaded::ChunkPos>,
    ) -> anyhow::Result<BTreeSet<native_world::loaded::ChunkPos>> {
        use anyhow::Context;
        self.ensure_rain_resolved()?;
        let mut chunks: BTreeSet<_> = saved_uuids.values().copied().collect();
        for entity in self
            .entities
            .iter()
            .chain(self.removed_saved.iter().map(|removed| &removed.entity))
        {
            chunks.insert(
                created_chunk(entity.item.position).context("item has an invalid position")?,
            );
            if let Some(created) = &entity.created {
                chunks.insert(
                    created_chunk(created.origin).context("created item has an invalid origin")?,
                );
                chunks.extend(created.visited.iter().copied());
            }
        }
        for merge in &self.saved_merges {
            for participant in [&merge.first, &merge.second] {
                chunks.insert(
                    created_chunk(participant.position)
                        .context("merged item has an invalid position")?,
                );
            }
        }
        chunks.extend(self.removed_created_chunks.iter().copied());
        for removed in &self.removed_created {
            chunks.extend(removed.chunks.iter().copied());
        }
        for pickup in &self.created_pickups {
            chunks.extend(pickup.chunks.iter().copied());
        }
        self.validate_item_chunk_group(&chunks, saved_uuids)?;
        self.validate_removed_item_chunk_group(&chunks, saved_uuids)?;
        Ok(chunks)
    }

    pub(crate) fn has_in_chunk(&self, pos: native_world::loaded::ChunkPos) -> bool {
        let chunk = BTreeSet::from([pos]);
        self.entities
            .iter()
            .any(|entity| Self::active_in(&chunk, entity.item.position))
    }

    pub(crate) fn push_from_soil_expansion(&mut self, block: IVec3) {
        for entity in &mut self.entities {
            let before = entity.item.position;
            if !Self::active(&self.active_chunks, before) {
                continue;
            }
            let Some(delta) =
                native_world::movement::soil_expansion_push(item_bounds(before), block)
            else {
                continue;
            };
            entity.item.position.y += delta;
            entity.item.previous_position = entity.item.position;
            if item_section_order(before) != item_section_order(entity.item.position) {
                entity.section_sequence = self.next_section_sequence;
                self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
            }
        }
    }

    /// A merge must save both original chunks and both participants' merge and
    /// final chunks together. This also covers partial merges across borders.
    fn validate_saved_merges(
        &self,
        positions: &BTreeSet<native_world::loaded::ChunkPos>,
        saved_uuids: &std::collections::BTreeMap<[i32; 4], native_world::loaded::ChunkPos>,
    ) -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        use native_world::loaded::ChunkPos;
        if self.saved_merges.is_empty() {
            return Ok(());
        }
        let chunk = |position: DVec3| -> anyhow::Result<ChunkPos> {
            ensure!(
                position.is_finite()
                    && position.x.abs() <= 30_000_512.0
                    && position.z.abs() <= 30_000_512.0,
                "merged item has an invalid position"
            );
            Ok(ChunkPos {
                x: (position.x.floor() as i32).div_euclid(16),
                z: (position.z.floor() as i32).div_euclid(16),
            })
        };
        let mut final_chunks = std::collections::BTreeMap::new();
        for entity in self
            .entities
            .iter()
            .chain(self.removed_saved.iter().map(|removed| &removed.entity))
        {
            if let Some(source) = &entity._saved_source
                && let Some(uuid) = crate::saved_item::optional_uuid(source, "UUID")?
            {
                ensure!(
                    final_chunks
                        .insert(uuid, chunk(entity.item.position)?)
                        .is_none(),
                    "duplicate live or removed item UUID"
                );
            }
        }
        for event in &self.saved_merges {
            let first = crate::saved_item::optional_uuid(&event.first.source, "UUID")?
                .context("merged item has no UUID")?;
            let second = crate::saved_item::optional_uuid(&event.second.source, "UUID")?
                .context("merged item has no UUID")?;
            ensure!(first != second, "merged items have duplicate UUIDs");
            let affected = [
                *saved_uuids
                    .get(&first)
                    .context("merged item source chunk is unavailable")?,
                chunk(event.first.position)?,
                *final_chunks
                    .get(&first)
                    .context("merged item final state is unavailable")?,
                *saved_uuids
                    .get(&second)
                    .context("merged item source chunk is unavailable")?,
                chunk(event.second.position)?,
                *final_chunks
                    .get(&second)
                    .context("merged item final state is unavailable")?,
            ];
            if affected.iter().any(|pos| positions.contains(pos)) {
                ensure!(
                    affected.iter().all(|pos| positions.contains(pos)),
                    "saved item merge requires all affected chunks"
                );
            }
        }
        Ok(())
    }

    pub(crate) fn validate_item_chunk_group(
        &self,
        positions: &BTreeSet<native_world::loaded::ChunkPos>,
        saved_uuids: &std::collections::BTreeMap<[i32; 4], native_world::loaded::ChunkPos>,
    ) -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        self.ensure_rain_resolved()?;
        ensure!(
            !self.uncoordinated_saved_merge,
            "saved item merge effects need coordinated saving"
        );
        self.validate_saved_merges(positions, saved_uuids)?;
        let mut seen = BTreeSet::new();
        for entity in &self.entities {
            let current = entity.item.position;
            ensure!(
                current.is_finite()
                    && current.x.abs() <= 30_000_512.0
                    && current.z.abs() <= 30_000_512.0,
                "item has an invalid position"
            );
            let destination = native_world::loaded::ChunkPos {
                x: (current.x.floor() as i32).div_euclid(16),
                z: (current.z.floor() as i32).div_euclid(16),
            };
            let uuid = entity
                ._saved_source
                .as_ref()
                .map(|source| crate::saved_item::optional_uuid(source, "UUID"))
                .transpose()?
                .flatten();
            let source = uuid.and_then(|uuid| saved_uuids.get(&uuid).copied());
            let origin = entity.created.as_ref().map(|created| created.origin);
            if let Some(origin) = origin {
                ensure!(
                    origin.is_finite()
                        && origin.x.abs() <= 30_000_512.0
                        && origin.z.abs() <= 30_000_512.0,
                    "created item has an invalid origin"
                );
            }
            let origin_chunk = origin.map(|origin| native_world::loaded::ChunkPos {
                x: (origin.x.floor() as i32).div_euclid(16),
                z: (origin.z.floor() as i32).div_euclid(16),
            });
            if !positions.contains(&destination)
                && !source.is_some_and(|pos| positions.contains(&pos))
                && !origin_chunk.is_some_and(|pos| positions.contains(&pos))
                && entity
                    .created
                    .as_ref()
                    .is_none_or(|created| created.visited.is_disjoint(positions))
            {
                continue;
            }
            if let Some(created) = &entity.created {
                ensure!(
                    entity._saved_source.is_none(),
                    "created item has a saved source"
                );
                let uuid = created.uuid.context("new item UUID is unavailable")?;
                ensure!(
                    created.thrower != CreatedThrower::Unknown,
                    "new item thrower is unknown"
                );
                ensure!(seen.insert(uuid), "duplicate live item UUID");
                ensure!(
                    self.player_uuid != Some(uuid),
                    "new item UUID conflicts with the player"
                );
                ensure!(
                    !saved_uuids.contains_key(&uuid),
                    "new item UUID conflicts with a saved item"
                );
                ensure!(
                    origin_chunk.is_some_and(|pos| positions.contains(&pos)),
                    "new item origin chunk is unavailable"
                );
                ensure!(
                    positions.contains(&destination),
                    "new item destination chunk is unavailable"
                );
                ensure!(
                    created.visited.is_subset(positions),
                    "new item path chunk is unavailable"
                );
                continue;
            }
            let uuid = uuid.context("new item needs coordinated creation")?;
            ensure!(seen.insert(uuid), "duplicate live item UUID");
            ensure!(
                source.is_some_and(|pos| positions.contains(&pos)),
                "item source chunk is unavailable"
            );
            ensure!(
                positions.contains(&destination),
                "item destination chunk is unavailable"
            );
        }
        Ok(())
    }

    pub(crate) fn validate_item_chunk_ownership(
        &self,
        pos: native_world::loaded::ChunkPos,
        saved_uuids: &std::collections::BTreeMap<[i32; 4], native_world::loaded::ChunkPos>,
    ) -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        self.ensure_rain_resolved()?;
        ensure!(
            !self.uncoordinated_saved_merge,
            "saved item merge effects need coordinated saving"
        );
        ensure!(
            !self.invalid_removed_created_position
                && !self.removed_created_chunks.contains(&pos)
                && self
                    .removed_created
                    .iter()
                    .all(|item| !item.chunks.contains(&pos)),
            "created item removals require coordinated chunk drafts"
        );
        self.validate_saved_merges(&BTreeSet::from([pos]), saved_uuids)?;
        let mut seen = BTreeSet::new();
        for entity in &self.entities {
            let current = entity.item.position;
            ensure!(
                current.is_finite()
                    && current.x.abs() <= 30_000_512.0
                    && current.z.abs() <= 30_000_512.0,
                "item has an invalid position"
            );
            if (current.x.floor() as i32).div_euclid(16) != pos.x
                || (current.z.floor() as i32).div_euclid(16) != pos.z
            {
                continue;
            }
            let source = entity
                ._saved_source
                .as_ref()
                .context("new item needs coordinated creation")?;
            let uuid = crate::saved_item::optional_uuid(source, "UUID")?
                .context("item has no stable UUID")?;
            ensure!(seen.insert(uuid), "duplicate live item UUID");
            ensure!(
                saved_uuids.get(&uuid) == Some(&pos),
                "item moved from another chunk"
            );
        }
        Ok(())
    }

    pub(crate) fn draft_saved_item(
        &self,
        uuid: [i32; 4],
    ) -> anyhow::Result<Option<crate::saved_item::ItemDraft>> {
        self.ensure_rain_resolved()?;
        for entity in &self.entities {
            let Some(source) = &entity._saved_source else {
                continue;
            };
            if crate::saved_item::optional_uuid(source, "UUID")? != Some(uuid) {
                continue;
            }
            return Ok(Some(Self::draft_item_entity(entity)?));
        }
        Ok(None)
    }

    /// Produce documents only for fresh entities whose base fields are still
    /// known. A chunk transaction must separately validate UUIDs and effects.
    pub(crate) fn draft_created_items_in_chunk(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> anyhow::Result<Vec<crate::saved_item::CreatedItemDraft>> {
        self.draft_created_items_in_chunks(&BTreeSet::from([pos]))
    }

    /// Traverse live items once even when a transaction contains many chunks.
    pub(crate) fn draft_created_items_in_chunks(
        &self,
        positions: &BTreeSet<native_world::loaded::ChunkPos>,
    ) -> anyhow::Result<Vec<crate::saved_item::CreatedItemDraft>> {
        use anyhow::{Context, ensure};
        self.ensure_rain_resolved()?;
        let mut result = Vec::new();
        let mut seen = BTreeSet::new();
        for entity in &self.entities {
            let Some(created) = &entity.created else {
                continue;
            };
            ensure!(
                created.origin.is_finite() && entity.item.position.is_finite(),
                "created item position is invalid"
            );
            let in_origin = Self::active_in(positions, created.origin);
            let in_current = Self::active_in(positions, entity.item.position);
            if !in_origin && !in_current && created.visited.is_disjoint(positions) {
                continue;
            }
            ensure!(
                in_origin && in_current,
                "created item crossed a chunk boundary"
            );
            ensure!(
                created.visited.is_subset(positions),
                "created item path chunk is unavailable"
            );
            ensure!(
                created.base_valid && entity.item.position == created.verified_position,
                "created item base fields require tick simulation"
            );
            let uuid = created.uuid.context("created item UUID is unavailable")?;
            ensure!(seen.insert(uuid), "duplicate created item UUID");
            let CreatedThrower::Known(thrower) = created.thrower else {
                anyhow::bail!("created item thrower is unknown")
            };
            let state = crate::saved_item::ItemSaveState {
                position: entity.item.position,
                velocity: entity.velocity,
                on_ground: entity.on_ground,
                no_gravity: entity.no_gravity,
                age: entity
                    .lifetime
                    .try_into()
                    .context("item age cannot be saved as a short")?,
                pickup_delay: entity
                    .pickup_delay
                    .try_into()
                    .context("pickup delay cannot be saved as a short")?,
                health: entity
                    ._damage_state
                    .health
                    .try_into()
                    .context("item health cannot be saved as a short")?,
                fire_ticks: entity
                    ._damage_state
                    .fire_ticks
                    .try_into()
                    .context("fire duration cannot be saved as a short")?,
                invulnerable: entity._damage_state.invulnerable,
                invulnerable_time: entity._damage_state.invulnerable_time,
                stack: &entity.item.stack,
            };
            result.push(crate::saved_item::draft_created(
                uuid,
                entity.owner,
                thrower,
                crate::saved_item::CreatedItemBase {
                    rotation: [created.yaw, 0.0],
                    fall_distance: created.fall_distance,
                    air: 300,
                    portal_cooldown: 0,
                },
                state,
            )?);
        }
        Ok(result)
    }

    fn draft_item_entity(entity: &ItemEntity) -> anyhow::Result<crate::saved_item::ItemDraft> {
        use anyhow::{Context, ensure};
        let source = entity
            ._saved_source
            .as_ref()
            .context("item has no saved source")?;
        let saved = crate::saved_item::read(source)?.context("source is not an item")?;
        ensure!(entity.owner == saved.owner, "item owner changed");
        let original = native_persistence::nbt::NamedTag {
            name: "".into(),
            value: (**source).clone(),
        };
        let state = crate::saved_item::ItemSaveState {
            position: entity.item.position,
            velocity: entity.velocity,
            on_ground: entity.on_ground,
            no_gravity: entity.no_gravity,
            age: entity
                .lifetime
                .try_into()
                .context("item age cannot be saved as a short")?,
            pickup_delay: entity
                .pickup_delay
                .try_into()
                .context("pickup delay cannot be saved as a short")?,
            health: entity
                ._damage_state
                .health
                .try_into()
                .context("item health cannot be saved as a short")?,
            fire_ticks: entity
                ._damage_state
                .fire_ticks
                .try_into()
                .context("fire duration cannot be saved as a short")?,
            invulnerable: entity._damage_state.invulnerable,
            invulnerable_time: entity._damage_state.invulnerable_time,
            stack: &entity.item.stack,
        };
        crate::saved_item::draft(&original, state)
    }

    pub(crate) fn draft_removed_item(
        &self,
        uuid: [i32; 4],
    ) -> anyhow::Result<Option<crate::saved_item::ItemDraft>> {
        use anyhow::ensure;
        self.ensure_rain_resolved()?;
        let mut found = None;
        for removed in &self.removed_saved {
            let entity = &removed.entity;
            let Some(source) = &entity._saved_source else {
                continue;
            };
            if crate::saved_item::optional_uuid(source, "UUID")? == Some(uuid) {
                ensure!(found.is_none(), "duplicate removed item UUID");
                found = Some(Self::draft_item_entity(entity)?);
            }
        }
        Ok(found)
    }

    pub(crate) fn validate_removed_item_chunk_group(
        &self,
        positions: &BTreeSet<native_world::loaded::ChunkPos>,
        saved_uuids: &std::collections::BTreeMap<[i32; 4], native_world::loaded::ChunkPos>,
    ) -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        self.ensure_rain_resolved()?;
        ensure!(
            !self.uncoordinated_saved_merge,
            "saved item merge effects need coordinated saving"
        );
        self.validate_saved_merges(positions, saved_uuids)?;
        ensure!(
            !self.invalid_removed_created_position,
            "removed created item has an invalid position"
        );
        ensure!(
            self.removed_created_chunks.is_disjoint(positions),
            "removed created item needs coordinated lifecycle saving"
        );
        let mut seen = BTreeSet::new();
        let live_created: BTreeSet<_> = if self.removed_created.is_empty() {
            BTreeSet::new()
        } else {
            self.entities
                .iter()
                .filter_map(|entity| entity.created.as_ref()?.uuid)
                .collect()
        };
        for removed in &self.removed_created {
            if removed.chunks.is_disjoint(positions) {
                continue;
            }
            ensure!(
                removed.chunks.is_subset(positions),
                "removed created item requires every visited chunk"
            );
            ensure!(
                seen.insert(removed.uuid),
                "duplicate removed created item UUID"
            );
            ensure!(
                !saved_uuids.contains_key(&removed.uuid),
                "removed created item UUID conflicts with a saved item"
            );
            ensure!(
                self.player_uuid != Some(removed.uuid),
                "removed created item UUID conflicts with the player"
            );
            ensure!(
                !live_created.contains(&removed.uuid),
                "removed created item UUID is still live"
            );
        }
        let mut live = BTreeSet::new();
        if !self.removed_saved.is_empty() {
            for entity in &self.entities {
                if let Some(source) = &entity._saved_source
                    && let Some(uuid) = crate::saved_item::optional_uuid(source, "UUID")?
                {
                    live.insert(uuid);
                }
            }
        }
        for removed in &self.removed_saved {
            let entity = &removed.entity;
            let source = entity
                ._saved_source
                .as_ref()
                .context("removed item has no source")?;
            let uuid = crate::saved_item::optional_uuid(source, "UUID")?
                .context("removed item has no UUID")?;
            let source_chunk = saved_uuids
                .get(&uuid)
                .copied()
                .context("removed item source chunk is unavailable")?;
            let position = entity.item.position;
            ensure!(
                position.is_finite()
                    && position.x.abs() <= 30_000_512.0
                    && position.z.abs() <= 30_000_512.0,
                "removed item has an invalid position"
            );
            let destination = native_world::loaded::ChunkPos {
                x: (position.x.floor() as i32).div_euclid(16),
                z: (position.z.floor() as i32).div_euclid(16),
            };
            if !positions.contains(&source_chunk) && !positions.contains(&destination) {
                continue;
            }
            ensure!(!live.contains(&uuid), "removed item UUID is still live");
            ensure!(seen.insert(uuid), "duplicate removed item UUID");
            ensure!(
                positions.contains(&source_chunk) && positions.contains(&destination),
                "removed item requires source and final chunks"
            );
        }
        Ok(())
    }

    /// These effects must be coordinated with player and world changes before
    /// the corresponding entity-chunk drafts can be written.
    pub(crate) fn created_removal_effects(
        &self,
        positions: &BTreeSet<native_world::loaded::ChunkPos>,
    ) -> Vec<([i32; 4], CreatedRemovalCause)> {
        self.removed_created
            .iter()
            .filter(|item| !item.chunks.is_disjoint(positions))
            .map(|item| (item.uuid, item.cause))
            .collect()
    }

    /// Only verified changes and successful pickup attempts are reported; a
    /// caller must still validate complete chunk drafts and coordinate saves.
    pub(crate) fn created_pickup_effects(
        &self,
        positions: &BTreeSet<native_world::loaded::ChunkPos>,
    ) -> Vec<CreatedItemPickup> {
        self.created_pickups
            .iter()
            .filter(|pickup| !pickup.chunks.is_disjoint(positions))
            .map(|pickup| pickup.effect.clone())
            .collect()
    }

    pub(crate) fn drain_pickup_sounds(&mut self) -> impl Iterator<Item = DVec3> + '_ {
        self.pickup_sounds.drain(..)
    }

    pub(crate) fn drain_extinguish_sounds(
        &mut self,
    ) -> impl Iterator<Item = crate::ExtinguishSound> + '_ {
        self.extinguish_sounds.drain(..)
    }

    #[cfg(test)]
    fn take_pickup_sounds(&mut self) -> Vec<DVec3> {
        self.drain_pickup_sounds().collect()
    }

    #[cfg(test)]
    pub(crate) fn debug_damage_states(&self) -> Vec<(i32, i32)> {
        self.entities
            .iter()
            .map(|item| (item._damage_state.health, item._damage_state.fire_ticks))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn debug_created_throwers(&self) -> Vec<Option<Option<[i32; 4]>>> {
        self.entities
            .iter()
            .map(
                |entity| match entity.created.as_ref().map(|created| created.thrower) {
                    Some(CreatedThrower::Known(thrower)) => Some(thrower),
                    _ => None,
                },
            )
            .collect()
    }
    pub fn set_damage_rules(
        &mut self,
        items: Arc<native_resources::item_registry::ItemRegistry>,
        damage_types: Arc<native_resources::enchantments::OxygenEnchantments>,
    ) {
        self.damage_rules = Some((items, damage_types));
    }

    pub fn damage_resistance(
        &self,
        stack: &ItemStack,
        damage: &str,
    ) -> anyhow::Result<Option<bool>> {
        let Some((items, damage_types)) = &self.damage_rules else {
            return Ok(None);
        };
        damage_resistant_in_world(stack, items, damage_types, damage)
    }
    pub fn set_player_uuid(&mut self, uuid: Option<[i32; 4]>) {
        self.player_uuid = uuid;
    }

    pub fn set_active_chunks(&mut self, chunks: BTreeSet<native_world::loaded::ChunkPos>) {
        if self.active_chunks.is_some() {
            let mut restored: Vec<_> = self
                .entities
                .iter()
                .enumerate()
                .filter(|(_, entity)| {
                    !Self::active(&self.active_chunks, entity.item.position)
                        && Self::active_in(&chunks, entity.item.position)
                })
                .map(|(index, entity)| (entity.section_sequence, index))
                .collect();
            restored.sort_unstable();
            for (_, index) in restored {
                // A reloaded Java entity runs the base ItemEntity constructor,
                // then restores its saved fields without rerolling velocity.
                let (_, bob_offset, _, random) = self.item_visuals();
                let new_id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1);
                let entity = &mut self.entities[index];
                // Java creates a fresh entity from NBT on chunk reload. Its
                // lifetime is saved, but Entity.tickCount is not.
                entity.item.age = 0;
                entity.item.previous_position = entity.item.position;
                entity.item.bob_offset = bob_offset;
                entity.id = new_id;
                entity.random = random;
                entity.ground_without_support = false;
                entity.section_sequence = self.next_section_sequence;
                self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
            }
        }
        self.active_chunks = Some(chunks);
    }

    fn active(chunks: &Option<BTreeSet<native_world::loaded::ChunkPos>>, position: DVec3) -> bool {
        chunks
            .as_ref()
            .is_none_or(|chunks| Self::active_in(chunks, position))
    }

    pub(crate) fn active_in(
        chunks: &BTreeSet<native_world::loaded::ChunkPos>,
        position: DVec3,
    ) -> bool {
        let x = (position.x / 16.0).floor();
        let z = (position.z / 16.0).floor();
        x >= f64::from(i32::MIN)
            && x <= f64::from(i32::MAX)
            && z >= f64::from(i32::MIN)
            && z <= f64::from(i32::MAX)
            && chunks.contains(&native_world::loaded::ChunkPos {
                x: x as i32,
                z: z as i32,
            })
    }

    pub fn spawn_saved(&mut self, saved: crate::saved_item::SavedItemState) {
        let position = saved.position;
        let (_, bob_offset, _, entity_random) = self.item_visuals();
        self.entities.push(ItemEntity {
            item: DroppedItem {
                entity_id: 0,
                item_age: 0,
                position,
                previous_position: position,
                stack: saved.stack,
                age: 0,
                bob_offset,
            },
            velocity: saved.velocity,
            on_ground: saved.on_ground,
            ground_without_support: false,
            pickup_delay: i32::from(saved.pickup_delay),
            lifetime: i32::from(saved.age),
            no_gravity: saved.no_gravity,
            owner: saved.owner,
            _saved_source: Some(saved.source),
            created: None,
            id: self.next_id,
            section_sequence: self.next_section_sequence,
            random: entity_random,
            _damage_state: ItemDamageState {
                health: i32::from(saved.health),
                fire_ticks: i32::from(saved.fire_ticks),
                invulnerable: saved.invulnerable,
                invulnerable_time: saved.invulnerable_time,
            },
        });
        self.next_id = self.next_id.wrapping_add(1);
        self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
    }

    /// ItemEntity created by Entity.spawnAtLocation. The random stream is
    /// still session-local; the saved Java entity RNG is not yet reproduced.
    pub(crate) fn spawn_from_entity(&mut self, position: DVec3, stack: ItemStack) {
        let (uuid, bob_offset, yaw, mut entity_random) = self.item_visuals();
        let velocity = DVec3::new(
            entity_random.next_double() * 0.2 - 0.1,
            0.2,
            entity_random.next_double() * 0.2 - 0.1,
        );
        self.entities.push(ItemEntity {
            item: DroppedItem {
                entity_id: 0,
                item_age: 0,
                position,
                previous_position: position,
                stack,
                age: 0,
                bob_offset,
            },
            velocity,
            on_ground: false,
            ground_without_support: false,
            pickup_delay: 10,
            lifetime: 0,
            no_gravity: false,
            owner: None,
            _saved_source: None,
            created: Some(CreatedItemIdentity::new(
                position,
                CreatedThrower::Known(None),
                yaw,
                uuid,
            )),
            id: self.next_id,
            section_sequence: self.next_section_sequence,
            random: entity_random,
            _damage_state: ItemDamageState::default(),
        });
        self.next_id = self.next_id.wrapping_add(1);
        self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
    }

    fn item_visuals(&mut self) -> (Option<[i32; 4]>, f32, f32, LegacyRandom) {
        if !self.entity_seed_checked {
            self.entity_seed_checked = true;
            let mut seed = [0_u8; 8];
            if getrandom::fill(&mut seed).is_ok() {
                self.entity_seeder = Some(native_core::random::XoroshiroRandom::new(
                    i64::from_ne_bytes(seed),
                ));
            }
        }
        let seed = self.entity_seeder.as_mut().map(|random| random.next_long());
        let fallback =
            || native_core::random::XoroshiroRandom::new(i64::from(self.next_id)).next_long();
        let (uuid, bob, yaw, random) = item_random_from_seed(seed.unwrap_or_else(fallback));
        (seed.map(|_| uuid), bob, yaw, random)
    }

    pub fn snapshot(&self) -> Arc<[DroppedItem]> {
        self.entities
            .iter()
            .filter(|entity| Self::active(&self.active_chunks, entity.item.position))
            .map(|entity| {
                let mut item = entity.item.clone();
                item.entity_id = entity.id as i32;
                item.item_age = entity.lifetime;
                item
            })
            .collect()
    }

    /// `Containers.dropContents`: even an empty slot consumes its location
    /// samples; all fragments from one slot use that same location.
    pub(crate) fn drop_container_contents_with_random(
        &mut self,
        position: IVec3,
        slots: &[Option<ItemStack>],
        random: &mut LegacyRandom,
    ) {
        for slot in slots {
            let location = DVec3::new(
                f64::from(position.x) + random.next_double() * 0.75 + 0.125,
                f64::from(position.y) + random.next_double() * 0.75,
                f64::from(position.z) + random.next_double() * 0.75 + 0.125,
            );
            let Some(stack) = slot else { continue };
            let mut remaining = stack.count();
            while remaining > 0 {
                let count =
                    remaining.min((random.next_int_bound(21).expect("positive bound") + 10) as u32);
                remaining -= count;
                let triangle = |random: &mut LegacyRandom, center: f64| {
                    center + 0.11485000171139836 * (random.next_double() - random.next_double())
                };
                let velocity = DVec3::new(
                    triangle(random, 0.0),
                    triangle(random, 0.2),
                    triangle(random, 0.0),
                );
                let (uuid, bob_offset, yaw, mut entity_random) = self.item_visuals();
                // ItemEntity's default constructor samples velocity before
                // Containers replaces it with the scatter velocity above.
                entity_random.next_double();
                entity_random.next_double();
                self.entities.push(ItemEntity {
                    item: DroppedItem {
                        entity_id: 0,
                        item_age: 0,
                        position: location,
                        previous_position: location,
                        stack: stack.with_count(count).expect("positive fragment"),
                        age: 0,
                        bob_offset,
                    },
                    velocity,
                    on_ground: false,
                    ground_without_support: false,
                    pickup_delay: 0,
                    lifetime: 0,
                    no_gravity: false,
                    owner: None,
                    _saved_source: None,
                    created: Some(CreatedItemIdentity::new(
                        location,
                        CreatedThrower::Known(None),
                        yaw,
                        uuid,
                    )),
                    id: self.next_id,
                    section_sequence: self.next_section_sequence,
                    random: entity_random,
                    _damage_state: ItemDamageState::default(),
                });
                self.next_id = self.next_id.wrapping_add(1);
                self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
            }
        }
    }

    /// Java scatters death drops instead of throwing them along the view.
    pub fn drop_death(
        &mut self,
        stack: ItemStack,
        origin: IVec3,
        feet: DVec3,
        player: PlayerState,
    ) {
        let speed = self.random.next_float() * 0.5_f32;
        let angle = self.random.next_float() * 6.2831855_f32;
        let velocity = DVec3::new(
            f64::from(-crate::physics::java_sin(f64::from(angle), false) * speed),
            f64::from(0.2_f32),
            f64::from(crate::physics::java_sin(f64::from(angle), true) * speed),
        );
        let position = origin.as_dvec3()
            + feet
            + DVec3::Y * (f64::from(player.pose.eye_height()) - f64::from(0.3_f32));
        let (uuid, bob_offset, yaw, entity_random) = self.item_visuals();
        self.entities.push(ItemEntity {
            item: DroppedItem {
                entity_id: 0,
                item_age: 0,
                position,
                previous_position: position,
                stack,
                age: 0,
                bob_offset,
            },
            velocity,
            on_ground: false,
            ground_without_support: false,
            pickup_delay: 40,
            lifetime: 0,
            no_gravity: false,
            owner: None,
            _saved_source: None,
            created: Some(CreatedItemIdentity::new(
                position,
                CreatedThrower::Known(None),
                yaw,
                uuid,
            )),
            id: self.next_id,
            section_sequence: self.next_section_sequence,
            random: entity_random,
            _damage_state: ItemDamageState::default(),
        });
        self.next_id = self.next_id.wrapping_add(1);
        self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
    }

    /// Throw a stack from the player's eye position. The current random stream
    /// is local to this experimental session, not yet the saved world's stream.
    pub fn drop_player(
        &mut self,
        stack: ItemStack,
        origin: IVec3,
        feet: DVec3,
        player: PlayerState,
        drop_origin: PlayerDropOrigin,
    ) {
        let angle = self.random.next_float() * 6.2831855_f32;
        let spread = 0.02_f32 * self.random.next_float();
        let vertical_spread = (self.random.next_float() - self.random.next_float()) * 0.1_f32;
        let pitch = (-player.pitch.to_degrees()) * 0.017453292_f32;
        let yaw = (player.yaw.to_degrees() - 180.0) * 0.017453292_f32;
        let sin_pitch = crate::physics::java_sin(f64::from(pitch), false);
        let cos_pitch = crate::physics::java_sin(f64::from(pitch), true);
        let sin_yaw = crate::physics::java_sin(f64::from(yaw), false);
        let cos_yaw = crate::physics::java_sin(f64::from(yaw), true);
        let velocity = DVec3::new(
            f64::from(-sin_yaw * cos_pitch * 0.3_f32) + f64::from(angle).cos() * f64::from(spread),
            f64::from(-sin_pitch * 0.3_f32 + 0.1_f32 + vertical_spread),
            f64::from(cos_yaw * cos_pitch * 0.3_f32) + f64::from(angle).sin() * f64::from(spread),
        );
        let position = origin.as_dvec3()
            + feet
            + DVec3::new(
                0.0,
                f64::from(player.pose.eye_height()) - f64::from(0.3_f32),
                0.0,
            );
        let (uuid, bob_offset, yaw, entity_random) = self.item_visuals();
        self.entities.push(ItemEntity {
            item: DroppedItem {
                entity_id: 0,
                item_age: 0,
                position,
                previous_position: position,
                stack,
                age: 0,
                bob_offset,
            },
            velocity,
            on_ground: false,
            ground_without_support: false,
            pickup_delay: 40,
            lifetime: 0,
            no_gravity: false,
            owner: None,
            _saved_source: None,
            created: Some(CreatedItemIdentity::new(
                position,
                match drop_origin {
                    PlayerDropOrigin::Direct => {
                        self.player_uuid.map_or(CreatedThrower::Unknown, |uuid| {
                            CreatedThrower::Known(Some(uuid))
                        })
                    }
                    PlayerDropOrigin::ContainerReturn => CreatedThrower::Known(None),
                },
                yaw,
                uuid,
            )),
            id: self.next_id,
            section_sequence: self.next_section_sequence,
            random: entity_random,
            _damage_state: ItemDamageState::default(),
        });
        self.next_id = self.next_id.wrapping_add(1);
        self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
    }

    pub fn drop_block(
        &mut self,
        position: IVec3,
        name: &str,
        hand: Option<&ItemStack>,
        mode: GameMode,
        enabled: bool,
        tables: Option<&BlockLootTables>,
    ) {
        self.drop_block_with_name(position, name, hand, mode, enabled, tables, None, None);
    }

    pub(crate) fn drop_block_with_name(
        &mut self,
        position: IVec3,
        name: &str,
        hand: Option<&ItemStack>,
        mode: GameMode,
        enabled: bool,
        tables: Option<&BlockLootTables>,
        custom_name: Option<&Tag>,
        properties: Option<mining::MiningProperties>,
    ) {
        let Some(material) = Material::from_block_name(name) else {
            return;
        };
        if !enabled
            || mode == GameMode::Creative
            || !properties
                .or_else(|| mining::mining_properties(material, hand))
                .is_some_and(|properties| properties.correct_for_drops)
        {
            return;
        }
        let Some(tables) = tables else { return };
        let enchantments = match hand.map(crate::inventory::item_enchantments).transpose() {
            Ok(enchantments) => enchantments.unwrap_or_default(),
            Err(error) => {
                eprintln!("unsupported loot enchantments for {name}: {error:#}");
                return;
            }
        };
        let tool = BasicTool {
            item: hand.map(ItemStack::id),
            count: hand.map_or(0, ItemStack::count),
            enchantments: &enchantments,
        };
        let drops = match tables.drops_with_random(name, tool, Some(&mut self.random)) {
            Ok(drops) => drops,
            Err(error) => {
                eprintln!("unsupported loot for {name}: {error:#}");
                return;
            }
        };
        self.spawn_drops(
            position,
            IVec3::ZERO,
            DropStrategy::FromMiddle,
            drops,
            custom_name,
            None,
        );
    }

    /// Block update destruction has no player tool or creative-mode exemption.
    pub fn drop_unattached_block(
        &mut self,
        position: IVec3,
        name: &str,
        enabled: bool,
        tables: Option<&BlockLootTables>,
    ) {
        if !enabled {
            return;
        }
        let Some(tables) = tables else { return };
        let tool = BasicTool {
            item: None,
            count: 0,
            enchantments: &[],
        };
        match tables.drops_with_random(name, tool, Some(&mut self.random)) {
            Ok(drops) => self.spawn_drops(
                position,
                IVec3::ZERO,
                DropStrategy::FromMiddle,
                drops,
                None,
                None,
            ),
            Err(error) => eprintln!("unsupported loot for {name}: {error:#}"),
        }
    }

    /// Evaluate non-entity loot before changing the block, so unsupported data
    /// packs cannot silently erase their drops.
    pub(crate) fn can_drop_block_without_entity(
        &self,
        name: &str,
        properties: &std::collections::BTreeMap<String, String>,
        tables: Option<&BlockLootTables>,
    ) -> bool {
        let Some(tables) = tables else {
            return false;
        };
        let mut random = self.random.clone();
        tables
            .drops_without_entity_at(name, Some(properties), Some(&mut random))
            .is_ok()
    }

    pub(crate) fn drop_block_without_entity(
        &mut self,
        position: IVec3,
        name: &str,
        properties: &std::collections::BTreeMap<String, String>,
        enabled: bool,
        tables: Option<&BlockLootTables>,
    ) -> anyhow::Result<()> {
        let tables = tables.ok_or_else(|| anyhow::anyhow!("block loot table unavailable"))?;
        // Commit the RNG only after the complete evaluation succeeds.
        let mut random = self.random.clone();
        let drops = tables.drops_without_entity_at(name, Some(properties), Some(&mut random))?;
        self.random = random;
        if enabled {
            self.spawn_drops(
                position,
                IVec3::ZERO,
                DropStrategy::FromMiddle,
                drops,
                None,
                None,
            );
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn prepare_block_entity_drops(
        &self,
        table: &str,
        block: &native_world::loaded::BlockState,
        hand: Option<&ItemStack>,
        entity_present: bool,
        components: &Tag,
        tables: Option<&BlockLootTables>,
    ) -> anyhow::Result<(Vec<Drop>, Vec<Tag>, LegacyRandom)> {
        let mut random = self.random.clone();
        let (drops, prepared) = Self::prepare_block_entity_drops_with_random(
            table,
            block,
            hand,
            entity_present,
            components,
            tables,
            &mut random,
        )?;
        Ok((drops, prepared, random))
    }

    pub(crate) fn prepare_block_entity_drops_with_random(
        table: &str,
        block: &native_world::loaded::BlockState,
        hand: Option<&ItemStack>,
        entity_present: bool,
        components: &Tag,
        tables: Option<&BlockLootTables>,
        random: &mut dyn native_resources::loot::LootRandom,
    ) -> anyhow::Result<(Vec<Drop>, Vec<Tag>)> {
        use anyhow::{Context, ensure};
        use native_resources::loot::LootBlockState;
        let tables = tables.context("block loot table unavailable")?;
        let enchantments = hand
            .map(crate::inventory::item_enchantments)
            .transpose()?
            .unwrap_or_default();
        let tool = BasicTool {
            item: hand.map(ItemStack::id),
            count: hand.map_or(0, ItemStack::count),
            enchantments: &enchantments,
        };
        ensure!(
            tables.fully_supported_block_entity_for_tool(table, tool),
            "unsupported block loot table"
        );
        let state = LootBlockState {
            name: &block.name,
            properties: &block.properties,
        };
        let drops =
            tables.drops_with_block_entity_at(table, &state, tool, entity_present, Some(random))?;
        let prepared = prepare_component_copies(&drops, components)?;
        Ok((drops, prepared))
    }

    pub(crate) fn validate_block_entity_drops(
        table: &str,
        components: &Tag,
        tables: Option<&BlockLootTables>,
    ) -> anyhow::Result<()> {
        use anyhow::Context;
        let tables = tables.context("block loot table unavailable")?;
        let drops = tables.potential_block_entity_drops(table)?;
        prepare_component_copies(&drops, components)?;
        Ok(())
    }

    /// `Block.popResource` uses the level RNG for the three position samples,
    /// including when the block-drops rule suppresses entity creation.
    pub(crate) fn drop_block_resources(
        &mut self,
        position: IVec3,
        drops: Vec<Drop>,
        components: Option<&[Tag]>,
        enabled: bool,
        random: &mut LegacyRandom,
    ) {
        self.spawn_level_resources(position, None, drops, components, enabled, random);
    }

    pub(crate) fn drop_interaction_with_level_random(
        &mut self,
        position: IVec3,
        face: IVec3,
        strategy: DropStrategy,
        drops: Vec<Drop>,
        enabled: bool,
        random: &mut LegacyRandom,
    ) {
        self.spawn_level_resources(
            position,
            (strategy == DropStrategy::ClickedFace).then_some(face),
            drops,
            None,
            enabled,
            random,
        );
    }

    fn spawn_level_resources(
        &mut self,
        position: IVec3,
        face: Option<IVec3>,
        drops: Vec<Drop>,
        components: Option<&[Tag]>,
        enabled: bool,
        random: &mut LegacyRandom,
    ) {
        for (index, drop) in drops.into_iter().enumerate() {
            let id = format!("{}:{}", drop.item.namespace, drop.item.path);
            let empty = Tag::Compound(vec![]);
            let component = components.map(|all| &all[index]).unwrap_or(&empty);
            let maximum = if components.is_some() {
                Some(
                    crate::inventory::saved_item_max(&id, component)
                        .expect("preflighted loot item"),
                )
            } else {
                ordinary_item_default_max(&id)
                    .or_else(|| native_world::Block::from_item_id(&id).map(|_| 64))
            };
            let mut remaining = drop.count;
            while remaining > 0 {
                let count = remaining.min(maximum.unwrap_or(99));
                remaining -= count;
                let offset = |axis: i32, random: &mut LegacyRandom| {
                    if axis == 0 {
                        -0.25 + random.next_double() * 0.5
                    } else {
                        f64::from(axis) * 0.625
                    }
                };
                let direction = face.unwrap_or(IVec3::ZERO);
                let location = DVec3::new(
                    f64::from(position.x) + 0.5 + offset(direction.x, random),
                    f64::from(position.y) + 0.5 + offset(direction.y, random) - 0.125,
                    f64::from(position.z) + 0.5 + offset(direction.z, random),
                );
                let face_velocity = face.map(|face| {
                    DVec3::new(
                        if face.x == 0 {
                            random.next_double() * 0.2 - 0.1
                        } else {
                            f64::from(face.x) * 0.1
                        },
                        if face.y == 0 {
                            random.next_double() * 0.1
                        } else {
                            f64::from(face.y) * 0.1 + 0.1
                        },
                        if face.z == 0 {
                            random.next_double() * 0.2 - 0.1
                        } else {
                            f64::from(face.z) * 0.1
                        },
                    )
                });
                if !enabled {
                    continue;
                }
                let stack = match maximum {
                    Some(maximum) => ItemStack::new(id.clone(), count, maximum, component.clone()),
                    None => ItemStack::unresolved(id.clone(), count, component.clone()),
                }
                .expect("validated loot item");
                let (uuid, bob_offset, yaw, mut entity_random) = self.item_visuals();
                let velocity = face_velocity.unwrap_or_else(|| {
                    DVec3::new(
                        entity_random.next_double() * 0.2 - 0.1,
                        0.2,
                        entity_random.next_double() * 0.2 - 0.1,
                    )
                });
                self.entities.push(ItemEntity {
                    item: DroppedItem {
                        entity_id: 0,
                        item_age: 0,
                        position: location,
                        previous_position: location,
                        stack,
                        age: 0,
                        bob_offset,
                    },
                    velocity,
                    on_ground: false,
                    ground_without_support: false,
                    pickup_delay: 10,
                    lifetime: 0,
                    no_gravity: false,
                    owner: None,
                    _saved_source: None,
                    created: Some(CreatedItemIdentity::new(
                        location,
                        CreatedThrower::Known(None),
                        yaw,
                        uuid,
                    )),
                    id: self.next_id,
                    section_sequence: self.next_section_sequence,
                    random: entity_random,
                    _damage_state: ItemDamageState::default(),
                });
                self.next_id = self.next_id.wrapping_add(1);
                self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
            }
        }
    }

    /// Spawn a previously evaluated block-interaction loot table.
    pub fn drop_interaction(
        &mut self,
        position: IVec3,
        face: IVec3,
        strategy: DropStrategy,
        drops: Vec<Drop>,
        enabled: bool,
    ) {
        if enabled {
            self.spawn_drops(position, face, strategy, drops, None, None);
        }
    }

    fn spawn_drops(
        &mut self,
        position: IVec3,
        face: IVec3,
        strategy: DropStrategy,
        drops: Vec<Drop>,
        custom_name: Option<&Tag>,
        prepared: Option<&[Tag]>,
    ) {
        for (index, drop) in drops.into_iter().enumerate() {
            let id = format!("{}:{}", drop.item.namespace, drop.item.path);
            let components = prepared.map(|all| all[index].clone()).unwrap_or_else(|| {
                Tag::Compound(if drop.copy_block_entity_custom_name {
                    custom_name
                        .map(|value| vec![("minecraft:custom_name".into(), value.clone())])
                        .unwrap_or_default()
                } else {
                    Vec::new()
                })
            });
            let maximum = if prepared.is_some() {
                crate::inventory::saved_item_max(&id, &components)
            } else {
                ordinary_item_default_max(&id)
                    .or_else(|| native_world::Block::from_item_id(&id).map(|_| 64))
            };
            let mut remaining = drop.count;
            while remaining > 0 {
                let count = remaining.min(maximum.unwrap_or(99));
                let stack = match maximum {
                    Some(max) => ItemStack::new(id.clone(), count, max, components.clone()),
                    None => ItemStack::unresolved(id.clone(), count, components.clone()),
                }
                .expect("validated loot stack");
                remaining -= count;
                let random = &mut self.random;
                let (location, velocity) = match strategy {
                    DropStrategy::FromMiddle => (
                        position.as_dvec3()
                            + DVec3::new(
                                0.5 + random.next_double() * 0.5 - 0.25,
                                0.375 + random.next_double() * 0.5 - 0.25,
                                0.5 + random.next_double() * 0.5 - 0.25,
                            ),
                        DVec3::new(
                            random.next_double() * 0.2 - 0.1,
                            0.2,
                            random.next_double() * 0.2 - 0.1,
                        ),
                    ),
                    DropStrategy::ClickedFace => {
                        let offset = |axis: i32, random: &mut LegacyRandom| {
                            if axis == 0 {
                                random.next_double() * 0.5 - 0.25
                            } else {
                                f64::from(axis) * 0.625
                            }
                        };
                        let location = position.as_dvec3()
                            + DVec3::new(
                                0.5 + offset(face.x, random),
                                0.375 + offset(face.y, random),
                                0.5 + offset(face.z, random),
                            );
                        let vx = if face.x == 0 {
                            random.next_double() * 0.2 - 0.1
                        } else {
                            f64::from(face.x) * 0.1
                        };
                        let vy = if face.y == 0 {
                            random.next_double() * 0.1
                        } else {
                            f64::from(face.y) * 0.1 + 0.1
                        };
                        let vz = if face.z == 0 {
                            random.next_double() * 0.2 - 0.1
                        } else {
                            f64::from(face.z) * 0.1
                        };
                        (location, DVec3::new(vx, vy, vz))
                    }
                };
                let (uuid, bob_offset, yaw, entity_random) = self.item_visuals();
                self.entities.push(ItemEntity {
                    item: DroppedItem {
                        entity_id: 0,
                        item_age: 0,
                        position: location,
                        previous_position: location,
                        stack,
                        age: 0,
                        bob_offset,
                    },
                    velocity,
                    on_ground: false,
                    ground_without_support: false,
                    pickup_delay: 10,
                    lifetime: 0,
                    no_gravity: false,
                    owner: None,
                    _saved_source: None,
                    created: Some(CreatedItemIdentity::new(
                        location,
                        CreatedThrower::Known(None),
                        yaw,
                        uuid,
                    )),
                    id: self.next_id,
                    section_sequence: self.next_section_sequence,
                    random: entity_random,
                    _damage_state: ItemDamageState::default(),
                });
                self.next_id = self.next_id.wrapping_add(1);
                self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
            }
        }
    }

    /// Advance simple airborne/solid-block movement and player pickup by one tick.
    #[allow(clippy::too_many_arguments)]
    pub fn tick<'a>(
        &mut self,
        origin: IVec3,
        player: DVec3,
        pose: PlayerPose,
        mode: GameMode,
        inventory: &mut Arc<PlayerInventory>,
        shape: impl FnMut(IVec3) -> &'a [Aabb],
        friction: impl FnMut(IVec3) -> f32,
    ) {
        self.tick_with_threshold(
            origin,
            player,
            pose,
            mode,
            inventory,
            shape,
            friction,
            |_| 0.0,
            |_| false,
            512,
            |_| native_world::fluid::FluidContacts::default(),
            false,
            |_, _, _| None,
            |_, _| false,
            |_| Some(false),
        );
    }

    /// The proof callback must certify that the complete movement area has no
    /// blocks or fluids with untracked base-entity effects.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tick_with_fluid_and_proof<'a>(
        &mut self,
        origin: IVec3,
        player: DVec3,
        pose: PlayerPose,
        mode: GameMode,
        inventory: &mut Arc<PlayerInventory>,
        shape: impl FnMut(IVec3) -> &'a [Aabb],
        friction: impl FnMut(IVec3) -> f32,
        bounce: impl FnMut(IVec3) -> f32,
        keep_support_height: impl FnMut(IVec3) -> bool,
        fluid: impl FnMut(BoundingBox) -> native_world::fluid::FluidContacts,
        fast_lava: bool,
        path: impl FnMut(BoundingBox, DVec3, DVec3) -> Option<Vec<native_world::fluid::FluidKind>>,
        safe: impl FnMut(BoundingBox, DVec3) -> bool,
        rain_at: impl FnMut(IVec3) -> Option<bool>,
    ) {
        self.tick_with_threshold(
            origin,
            player,
            pose,
            mode,
            inventory,
            shape,
            friction,
            bounce,
            keep_support_height,
            512,
            fluid,
            fast_lava,
            path,
            safe,
            rain_at,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn tick_with_threshold<'a>(
        &mut self,
        origin: IVec3,
        player: DVec3,
        pose: PlayerPose,
        mode: GameMode,
        inventory: &mut Arc<PlayerInventory>,
        mut shape: impl FnMut(IVec3) -> &'a [Aabb],
        mut friction: impl FnMut(IVec3) -> f32,
        mut bounce: impl FnMut(IVec3) -> f32,
        mut keep_support_height: impl FnMut(IVec3) -> bool,
        index_threshold: usize,
        mut fluid: impl FnMut(BoundingBox) -> native_world::fluid::FluidContacts,
        fast_lava: bool,
        mut path: impl FnMut(BoundingBox, DVec3, DVec3) -> Option<Vec<native_world::fluid::FluidKind>>,
        mut safe: impl FnMut(BoundingBox, DVec3) -> bool,
        mut rain_at: impl FnMut(IVec3) -> Option<bool>,
    ) {
        let player_box = movement::player_bounds(player, pose.height());
        let pickup = BoundingBox {
            min: player_box.min - DVec3::new(1.0, 0.5, 1.0),
            max: player_box.max + DVec3::new(1.0, 0.5, 1.0),
        };
        let player_uuid = self.player_uuid;
        let active_chunks = &self.active_chunks;
        let damage_rules = &self.damage_rules;
        let mut removed = vec![false; self.entities.len()];
        let mut created_removal_causes = vec![None; self.entities.len()];
        // For large populations, index the horizontal block cell. A one-cell
        // halo contains every possible merge candidate; section traversal
        // order is applied after the spatial search.
        let mut spatial: Option<HashMap<(i64, i64), Vec<usize>>> = None;
        let mut spatial_eligible = self.entities.len() >= index_threshold;
        for index in 0..self.entities.len() {
            if removed[index] {
                continue;
            }
            let entity = &mut self.entities[index];
            if !Self::active(active_chunks, entity.item.position) {
                continue;
            }
            if entity._damage_state.invulnerable_time > 0 {
                entity._damage_state.invulnerable_time -= 1;
            }
            if entity.pickup_delay > 0 && entity.pickup_delay != i32::from(i16::MAX) {
                entity.pickup_delay -= 1;
            }
            entity.item.previous_position = entity.item.position;
            let relative = entity.item.position - origin.as_dvec3();
            let bounds = item_bounds(relative);
            let contacts = fluid(bounds);
            if let Some(created) = &mut entity.created {
                if contacts.water.depth > 0.0 {
                    created.fall_distance = 0.0;
                }
                if contacts.lava.depth > 0.0 {
                    created.fall_distance *= 0.5;
                }
            }
            if let Some(rules) = damage_rules
                && apply_fire_tick(entity, rules, contacts.lava.depth > 0.0).unwrap_or(false)
            {
                removed[index] = true;
                continue;
            }
            entity.velocity = contacts.water.push_entity(entity.velocity, 0.014);
            entity.velocity = contacts.lava.push_entity(
                entity.velocity,
                if fast_lava {
                    0.007
                } else {
                    0.0023333333333333335
                },
            );
            let fluid_drag = if contacts.water.depth > f64::from(0.1_f32) {
                Some(f64::from(0.99_f32))
            } else if contacts.lava.depth > f64::from(0.1_f32) {
                Some(f64::from(0.95_f32))
            } else {
                None
            };
            if let Some(drag) = fluid_drag {
                entity.velocity.x *= drag;
                entity.velocity.z *= drag;
                if entity.velocity.y < f64::from(0.06_f32) {
                    entity.velocity.y += f64::from(0.0005_f32);
                }
            } else if !entity.no_gravity {
                entity.velocity.y -= 0.04;
            }
            let moves = !entity.on_ground
                || entity.velocity.x * entity.velocity.x + entity.velocity.z * entity.velocity.z
                    > f64::from(1.0e-5_f32)
                || entity
                    .item
                    .age
                    .wrapping_add(1)
                    .wrapping_add(entity.id)
                    .is_multiple_of(4);
            let requested = entity.velocity;
            let mut actual = DVec3::ZERO;
            if moves {
                actual =
                    movement::resolve_box(bounds, requested, entity.on_ground, 0.0, &mut shape);
                entity.item.position += actual;
                let hit_x = (requested.x - actual.x).abs() >= f64::from(1.0e-5_f32);
                let hit_z = (requested.z - actual.z).abs() >= f64::from(1.0e-5_f32);
                let hit_y = requested.y != actual.y;
                entity.on_ground = hit_y && requested.y < 0.0;
                let relative_position = entity.item.position - origin.as_dvec3();
                let support = if entity.on_ground {
                    let found = movement::supporting_box(
                        relative_position,
                        0.125,
                        actual,
                        entity.ground_without_support,
                        &mut shape,
                    );
                    entity.ground_without_support = found.is_none();
                    found
                } else {
                    entity.ground_without_support = false;
                    None
                };
                let below = movement::movement_on_block(
                    relative_position,
                    support,
                    &mut keep_support_height,
                );
                if let Some(created) = &mut entity.created {
                    if entity.on_ground {
                        created.fall_distance = 0.0;
                    } else if contacts.water.depth <= 0.0 && actual.y < 0.0 {
                        // Entity.checkFallDamage rounds the displacement to float.
                        created.fall_distance -= f64::from(actual.y as f32);
                    }
                }
                if hit_x {
                    entity.velocity.x = 0.0;
                }
                if hit_z {
                    entity.velocity.z = 0.0;
                }
                if hit_y {
                    entity.velocity.y = if entity.on_ground {
                        vertical_restitution(
                            entity.velocity.y,
                            actual.y,
                            f64::from(bounce(below) * 0.8_f32),
                            0.04,
                        )
                    } else {
                        0.0
                    };
                }
                let drag = 0.98_f32;
                let ground_drag = if entity.on_ground {
                    drag * friction(below)
                } else {
                    drag
                };
                entity.velocity *= DVec3::new(
                    f64::from(ground_drag),
                    f64::from(drag),
                    f64::from(ground_drag),
                );
                if entity.on_ground && entity.velocity.y < 0.0 {
                    entity.velocity.y *= -0.5;
                }
            }
            if let Some(created) = &mut entity.created {
                let supported = created.base_valid
                    && contacts.water.depth <= 0.0
                    && contacts.lava.depth <= 0.0
                    && entity._damage_state.fire_ticks == 0
                    && safe(bounds, actual);
                created.verify_movement(
                    entity.item.previous_position,
                    entity.item.position,
                    supported,
                );
            }
            let moved_block = entity.item.previous_position.floor() != entity.item.position.floor();
            let after = if actual == DVec3::ZERO {
                contacts
            } else {
                fluid(item_bounds(entity.item.position - origin.as_dvec3()))
            };
            let effects = path(bounds, actual, requested).unwrap_or_else(|| {
                let mut effects = Vec::new();
                if after.lava.depth > 0.0 {
                    effects.push(native_world::fluid::FluidKind::Lava);
                }
                if after.water.depth > 0.0 {
                    effects.push(native_world::fluid::FluidKind::Water);
                }
                effects
            });
            let was_burning = entity._damage_state.fire_ticks > 0
                && damage_rules.as_ref().is_some_and(|(items, tags)| {
                    damage_resistant_in_world(&entity.item.stack, items, tags, "minecraft:in_fire")
                        .ok()
                        .flatten()
                        == Some(false)
                });
            for effect in effects {
                match effect {
                    native_world::fluid::FluidKind::Lava => {
                        if let Some(rules) = damage_rules
                            && apply_lava_contact(entity, rules).unwrap_or(false)
                        {
                            removed[index] = true;
                            break;
                        }
                    }
                    native_world::fluid::FluidKind::Water => {
                        entity._damage_state.fire_ticks = entity._damage_state.fire_ticks.min(0)
                    }
                }
            }
            if !removed[index] && entity._damage_state.fire_ticks > 0 {
                match item_in_rain(entity.item.position - origin.as_dvec3(), &mut rain_at) {
                    Some(true) => entity._damage_state.fire_ticks = 0,
                    Some(false) => {}
                    None => self.unresolved_rain_effect = true,
                }
            }
            if removed[index] {
                continue;
            }
            if was_burning && entity._damage_state.fire_ticks <= 0 {
                let event = crate::ExtinguishSound {
                    position: entity.item.position,
                    pitch: crate::extinguish_pitch(&mut entity.random),
                };
                if self.extinguish_sounds.len() == 4096 {
                    self.extinguish_sounds.pop_front();
                }
                self.extinguish_sounds.push_back(event);
            }
            if item_section_order(entity.item.previous_position)
                != item_section_order(entity.item.position)
            {
                entity.section_sequence = self.next_section_sequence;
                self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
            }
            if let Some(cells) = &mut spatial {
                let previous = item_cell(entity.item.previous_position);
                let current = item_cell(entity.item.position);
                if previous != current {
                    if let Some(entries) = cells.get_mut(&previous) {
                        entries.retain(|other| *other != index);
                        if entries.is_empty() {
                            cells.remove(&previous);
                        }
                    }
                    cells.entry(current).or_default().push(index);
                }
            }
            let next_tick = entity.item.age.wrapping_add(1);
            if next_tick.is_multiple_of(if moved_block { 2 } else { 40 }) {
                if spatial.is_none() && spatial_eligible {
                    let mut cells = HashMap::<(i64, i64), Vec<usize>>::new();
                    for (entity_index, entity) in self.entities.iter().enumerate() {
                        if !removed[entity_index] {
                            cells
                                .entry(item_cell(entity.item.position))
                                .or_default()
                                .push(entity_index);
                        }
                    }
                    if cells
                        .values()
                        .any(|entries| entries.len() >= self.entities.len() / 2)
                    {
                        spatial_eligible = false;
                    } else {
                        spatial = Some(cells);
                    }
                }
                let candidates = spatial.as_ref().and_then(|cells| {
                    item_candidates(
                        cells,
                        self.entities[index].item.position,
                        self.entities.len() / 2,
                    )
                });
                let changes = Self::merge_neighbours(
                    &mut self.entities,
                    &mut removed,
                    active_chunks,
                    index,
                    candidates.as_deref(),
                );
                self.uncoordinated_saved_merge |= changes.unresolved;
                self.saved_merges.extend(changes.saved);
            }
            if removed[index] {
                continue;
            }
            let entity = &mut self.entities[index];
            entity.item.age = next_tick;
            if entity.lifetime != i32::from(i16::MIN) {
                entity.lifetime = entity.lifetime.saturating_add(1);
            }
            entity.velocity = after.water.push_entity(entity.velocity, 0.014);
            entity.velocity = after.lava.push_entity(
                entity.velocity,
                if fast_lava {
                    0.007
                } else {
                    0.0023333333333333335
                },
            );
            if entity.lifetime >= 6000 {
                removed[index] = true;
                created_removal_causes[index] = Some(CreatedRemovalCause::Expired);
                continue;
            }
            let bounds = item_bounds(entity.item.position - origin.as_dvec3());
            if entity.pickup_delay == 0
                && entity.owner.is_none_or(|owner| player_uuid == Some(owner))
                && mode != GameMode::Spectator
                && bounds.max.cmpgt(pickup.min).all()
                && bounds.min.cmplt(pickup.max).all()
            {
                let mut stack = Some(entity.item.stack.clone());
                let (pickup_succeeded, transferred) = Arc::make_mut(inventory)
                    .pickup_stack_with_transfer(&mut stack, mode == GameMode::Creative);
                let silent = entity
                    ._saved_source
                    .as_ref()
                    .and_then(|source| source.get("Silent"))
                    .and_then(crate::player_data::numeric_byte)
                    .is_some_and(|value| value != 0);
                if pickup_succeeded && !silent {
                    if self.pickup_sounds.len() == 4096 {
                        self.pickup_sounds.pop_front();
                    }
                    self.pickup_sounds.push_back(entity.item.position);
                }
                let discarded = entity.item.stack.count()
                    - transferred
                    - stack.as_ref().map_or(0, |s| s.count());
                if (transferred > 0 || pickup_succeeded)
                    && let Some(created) = &entity.created
                    && created.base_valid
                    && created.verified_position == entity.item.position
                    && created.thrower != CreatedThrower::Unknown
                    && let (Some(uuid), Some(origin_chunk), Some(destination)) = (
                        created.uuid,
                        created_chunk(created.origin),
                        created_chunk(entity.item.position),
                    )
                {
                    let mut chunks = created.visited.clone();
                    chunks.insert(origin_chunk);
                    chunks.insert(destination);
                    self.created_pickups.push(CreatedPickupRecord {
                        effect: CreatedItemPickup {
                            uuid,
                            original_stack: entity.item.stack.clone(),
                            transferred,
                            discarded,
                            pickup_succeeded,
                        },
                        chunks,
                    });
                }
                if let Some(remainder) = stack {
                    entity.item.stack = remainder;
                } else {
                    removed[index] = true;
                    created_removal_causes[index] = Some(CreatedRemovalCause::Pickup);
                    continue;
                }
            }
        }
        self.removed_saved.extend(
            self.entities
                .iter()
                .zip(&removed)
                .enumerate()
                .filter(|(_, (entity, removed))| **removed && entity._saved_source.is_some())
                .map(|(_, (entity, _))| RemovedSavedItem {
                    entity: entity.clone(),
                }),
        );
        for (index, (entity, removed)) in self.entities.iter().zip(&removed).enumerate() {
            if *removed && let Some(created) = &entity.created {
                if let (Some(cause), Some(uuid), Some(origin), Some(destination)) = (
                    created_removal_causes[index],
                    created.uuid,
                    created_chunk(created.origin),
                    created_chunk(entity.item.position),
                ) && created.base_valid
                    && created.verified_position == entity.item.position
                    && created.thrower != CreatedThrower::Unknown
                {
                    let mut chunks = created.visited.clone();
                    chunks.insert(origin);
                    chunks.insert(destination);
                    self.removed_created.push(RemovedCreatedItem {
                        uuid,
                        chunks,
                        cause,
                    });
                    continue;
                }
                self.removed_created_chunks
                    .extend(created.visited.iter().copied());
                for position in [created.origin, entity.item.position] {
                    if !position.is_finite()
                        || position.x.abs() > 30_000_512.0
                        || position.z.abs() > 30_000_512.0
                    {
                        self.invalid_removed_created_position = true;
                        continue;
                    }
                    self.removed_created_chunks
                        .insert(native_world::loaded::ChunkPos {
                            x: (position.x.floor() as i32).div_euclid(16),
                            z: (position.z.floor() as i32).div_euclid(16),
                        });
                }
            }
        }
        let mut index = 0;
        self.entities.retain(|_| {
            let keep = !removed[index];
            index += 1;
            keep
        });
    }

    fn merge_neighbours(
        entities: &mut [ItemEntity],
        removed: &mut [bool],
        active: &Option<BTreeSet<native_world::loaded::ChunkPos>>,
        index: usize,
        candidates: Option<&[usize]>,
    ) -> MergeChanges {
        fn eligible(entity: &ItemEntity) -> bool {
            entity.pickup_delay != i32::from(i16::MAX)
                && entity.lifetime != i32::from(i16::MIN)
                && entity.lifetime < 6000
                && entity
                    .item
                    .stack
                    .max_count()
                    .is_some_and(|max| entity.item.stack.count() < max)
        }
        if !eligible(&entities[index]) {
            return MergeChanges::default();
        }
        let mut changes = MergeChanges::default();
        let origin = entities[index].item.position;
        let mut nearby: Vec<_> = (0..candidates.map_or(entities.len(), <[usize]>::len))
            .map(|candidate| candidates.map_or(candidate, |items| items[candidate]))
            .filter(|other| {
                let delta = (origin - entities[*other].item.position).abs();
                *other != index
                    && !removed[*other]
                    && Self::active(active, entities[*other].item.position)
                    && delta.x < 0.75
                    && delta.y < 0.25
                    && delta.z < 0.75
            })
            .collect();
        nearby.sort_by_key(|other| {
            (
                item_section_order(entities[*other].item.position),
                entities[*other].section_sequence,
                *other,
            )
        });
        for other in nearby {
            if other == index
                || removed[other]
                || !Self::active(active, entities[other].item.position)
            {
                continue;
            }
            let (current, neighbor) = if index < other {
                let (left, right) = entities.split_at_mut(other);
                (&mut left[index], &mut right[0])
            } else {
                let (left, right) = entities.split_at_mut(index);
                (&mut right[0], &mut left[other])
            };
            let delta = (current.item.position - neighbor.item.position).abs();
            if delta.x >= 0.75
                || delta.y >= 0.25
                || delta.z >= 0.75
                || !eligible(neighbor)
                || current.owner != neighbor.owner
                || !stacks_mergeable(&current.item.stack, &neighbor.item.stack)
            {
                continue;
            }
            let (target, source, source_index) =
                if neighbor.item.stack.count() < current.item.stack.count() {
                    (current, neighbor, other)
                } else {
                    (neighbor, current, index)
                };
            let capacity = target
                .item
                .stack
                .max_count()
                .unwrap()
                .min(64)
                .saturating_sub(target.item.stack.count());
            let amount = capacity.min(source.item.stack.count());
            if amount > 0
                || target.pickup_delay < source.pickup_delay
                || target.lifetime > source.lifetime
            {
                match (&target._saved_source, &source._saved_source) {
                    (Some(first), Some(second)) => changes.saved.push(SavedItemMerge {
                        first: SavedMergeParticipant {
                            source: Arc::clone(first),
                            position: target.item.position,
                        },
                        second: SavedMergeParticipant {
                            source: Arc::clone(second),
                            position: source.item.position,
                        },
                    }),
                    (None, None) => {
                        // A partial merge can leave both new entities alive in
                        // different chunks. Until their histories are linked,
                        // saving either chunk independently could duplicate items.
                        changes.unresolved |= target.created.is_some() || source.created.is_some();
                    }
                    _ => changes.unresolved = true,
                }
            }
            if amount > 0 {
                target.item.stack = target
                    .item
                    .stack
                    .with_count(target.item.stack.count() + amount)
                    .expect("valid merged stack");
            }
            target.pickup_delay = target.pickup_delay.max(source.pickup_delay);
            target.lifetime = target.lifetime.min(source.lifetime);
            if amount == source.item.stack.count() {
                removed[source_index] = true;
                if source_index == index {
                    break;
                }
            } else if amount > 0 {
                source.item.stack = source
                    .item
                    .stack
                    .with_count(source.item.stack.count() - amount)
                    .expect("valid remainder");
            }
        }
        changes
    }
}

fn item_cell(position: DVec3) -> (i64, i64) {
    (position.x.floor() as i64, position.z.floor() as i64)
}

pub(crate) fn item_section_order(position: DVec3) -> (i64, u64, u64) {
    let x = (position.x / 16.0).floor() as i64;
    let y = (position.y / 16.0).floor() as i64;
    let z = (position.z / 16.0).floor() as i64;
    (
        x,
        (z as u64) & ((1 << 22) - 1),
        (y as u64) & ((1 << 20) - 1),
    )
}

fn item_candidates(
    cells: &HashMap<(i64, i64), Vec<usize>>,
    position: DVec3,
    limit: usize,
) -> Option<Vec<usize>> {
    let (x, z) = item_cell(position);
    let mut candidates = Vec::new();
    for dx in -1_i64..=1 {
        for dz in -1_i64..=1 {
            if let (Some(x), Some(z)) = (x.checked_add(dx), z.checked_add(dz))
                && let Some(entries) = cells.get(&(x, z))
            {
                candidates.extend_from_slice(entries);
                if candidates.len() >= limit {
                    return None;
                }
            }
        }
    }
    candidates.sort_unstable();
    Some(candidates)
}

fn item_bounds(position: DVec3) -> BoundingBox {
    BoundingBox {
        min: position + DVec3::new(-0.125, 0.0, -0.125),
        max: position + DVec3::new(0.125, 0.25, 0.125),
    }
}

// Entity constructs an insecure UUID before ItemEntity initializes its visuals.
// The actual seed and stored UUID are not available in a loaded world.
fn item_random_from_seed(seed: i64) -> ([i32; 4], f32, f32, LegacyRandom) {
    let mut random = LegacyRandom::new(seed);
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&random.next_long().to_be_bytes());
    bytes[8..].copy_from_slice(&random.next_long().to_be_bytes());
    let uuid = uuid_v4(bytes);
    let bob = random.next_float() * std::f32::consts::PI * 2.0;
    let yaw = random.next_float() * 360.0;
    (uuid, bob, yaw, random)
}

/// Entity.isInRain checks the block containing the feet and the upper edge.
fn item_in_rain(position: DVec3, rain_at: impl FnMut(IVec3) -> Option<bool>) -> Option<bool> {
    crate::weather::entity_in_rain(position, 0.25, rain_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::packs::ResourceProvider;

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn item_constructor_random_sequence_matches_java() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        native_resources::packs::ResourceStack::open(&game).unwrap();
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
            .map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ItemRandomOracle.java");
        let output = temp.path().join("item-random.json");
        std::fs::write(&source, include_str!("../tests/java/ItemRandomOracle.java")).unwrap();
        let run = |command: &mut std::process::Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "item constructor oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(std::process::Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source));
        run(std::process::Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("ItemRandomOracle")
            .arg(&output));
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        for row in rows.as_array().unwrap() {
            let (uuid, bob, yaw, mut random) = item_random_from_seed(row["seed"].as_i64().unwrap());
            let expected_uuid: Vec<i32> = row["uuid"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap() as i32)
                .collect();
            assert_eq!(uuid.as_slice(), expected_uuid);
            assert_eq!(bob.to_bits(), row["bob"].as_i64().unwrap() as i32 as u32);
            assert_eq!(yaw.to_bits(), row["yaw"].as_i64().unwrap() as i32 as u32);
            if row["mode"] == 0 {
                let x = random.next_double() * 0.2 - 0.1;
                let z = random.next_double() * 0.2 - 0.1;
                assert_eq!(x.to_bits(), row["x"].as_i64().unwrap() as u64);
                assert_eq!(z.to_bits(), row["z"].as_i64().unwrap() as u64);
            }
            assert_eq!(i64::from(random.next_int()), row["next"].as_i64().unwrap());
        }
    }

    #[test]
    fn item_rain_uses_both_positions_and_preserves_unknown() {
        let position = DVec3::new(-0.25, 2.8, -1.1);
        let feet = IVec3::new(-1, 2, -2);
        let top = IVec3::new(-1, 3, -2);
        assert_eq!(
            item_in_rain(position, |p| if p == feet { None } else { Some(p == top) }),
            Some(true)
        );
        assert_eq!(
            item_in_rain(position, |p| if p == feet { None } else { Some(false) }),
            None
        );
        assert_eq!(item_in_rain(position, |_| Some(false)), Some(false));
        assert_eq!(item_in_rain(DVec3::splat(f64::NAN), |_| Some(true)), None);
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn item_rain_follows_fire_damage_and_lava_effects() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = Arc::new(source.vanilla_item_registry().unwrap());
        let tags = Arc::new(
            native_resources::enchantments::OxygenEnchantments::load(&mut source).unwrap(),
        );
        for (rain, lava, expected_health, expected_fire) in [
            (Some(true), false, 9, 0),
            (None, false, 9, 19),
            (Some(true), true, 5, 0),
        ] {
            let mut items = ItemEntities::default();
            items.set_damage_rules(Arc::clone(&registry), Arc::clone(&tags));
            let position = DVec3::new(3.5, 5.8, 4.5);
            items.spawn_from_entity(
                position,
                ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap(),
            );
            let entity = &mut items.entities[0];
            entity.no_gravity = true;
            entity.velocity = DVec3::ZERO;
            entity._damage_state.health = 10;
            entity._damage_state.fire_ticks = 20;
            let mut expected_random = entity.random.clone();
            let expected_pitch = crate::extinguish_pitch(&mut expected_random);
            items.tick_with_fluid_and_proof(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Spectator,
                &mut Arc::new(PlayerInventory::default()),
                |_| &[],
                |_| 0.6,
                |_| 0.0,
                |_| false,
                |_| native_world::fluid::FluidContacts::default(),
                false,
                |_, _, _| {
                    Some(if lava {
                        vec![native_world::fluid::FluidKind::Lava]
                    } else {
                        vec![]
                    })
                },
                |_, _| true,
                |p| if p.y == 6 { rain } else { Some(false) },
            );
            assert_eq!(items.entities[0]._damage_state.health, expected_health);
            assert_eq!(items.entities[0]._damage_state.fire_ticks, expected_fire);
            assert_eq!(items.unresolved_rain_effect, rain.is_none());
            let sounds: Vec<_> = items.drain_extinguish_sounds().collect();
            assert_eq!(sounds.len(), usize::from(rain == Some(true)));
            if let Some(sound) = sounds.first() {
                assert_eq!(sound.position, position);
                assert_eq!(sound.pitch, expected_pitch);
                assert_eq!(
                    items.entities[0].random.next_int(),
                    expected_random.next_int()
                );
            }
            if rain.is_none() {
                assert!(items.affected_item_chunks(&Default::default()).is_err());
                assert!(items.draft_saved_item([0; 4]).is_err());
                assert!(items.draft_removed_item([0; 4]).is_err());
                assert!(
                    items
                        .draft_created_items_in_chunks(&BTreeSet::new())
                        .is_err()
                );
                items.entities.clear();
                assert!(items.affected_item_chunks(&Default::default()).is_err());
            }
        }
    }

    #[test]
    fn created_items_keep_random_uuid_and_original_chunk_when_streamed() {
        use native_world::loaded::ChunkPos;
        let mut items = ItemEntities::default();
        let stack = ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        let origin = DVec3::new(0.5, 70.0, 0.5);
        items.spawn_from_entity(origin, stack.clone());
        items.spawn_from_entity(origin, stack);
        let identity = items.entities[0].created.clone().unwrap();
        let uuid = identity.uuid.expect("OS randomness is available");
        assert_eq!((uuid[1] as u32 >> 12) & 0xf, 4);
        assert_eq!((uuid[2] as u32 >> 30) & 0x3, 2);
        assert_ne!(
            identity.uuid,
            items.entities[1].created.as_ref().unwrap().uuid
        );
        assert_eq!(identity.thrower, CreatedThrower::Known(None));
        items.entities.pop();
        items.entities[0].item.position.x = 32.5;
        let origin_chunk = ChunkPos { x: 0, z: 0 };
        let destination = ChunkPos { x: 2, z: 0 };
        items.set_active_chunks(BTreeSet::from([origin_chunk]));
        items.set_active_chunks(BTreeSet::from([destination]));
        assert_eq!(items.entities[0].created.as_ref(), Some(&identity));
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([origin_chunk]), &Default::default())
                .is_err()
        );
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([destination]), &Default::default())
                .is_err()
        );
    }

    #[test]
    fn uuid_generation_does_not_change_game_random_stream() {
        let bytes = [0xff; 16];
        let uuid = uuid_v4(bytes);
        assert_eq!(uuid, [-1, -45057, -1073741825, -1]);
        let mut first = ItemEntities::default();
        let mut second = ItemEntities::default();
        first.entity_seed_checked = true;
        second.entity_seed_checked = true;
        first.entity_seeder = Some(native_core::random::XoroshiroRandom::new(7));
        second.entity_seeder = Some(native_core::random::XoroshiroRandom::new(7));
        let stack = ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        first.spawn_from_entity(DVec3::ZERO, stack.clone());
        second.spawn_from_entity(DVec3::ZERO, stack);
        assert_eq!(first.entities[0].velocity, second.entities[0].velocity);
        assert_eq!(first.random.next_long(), second.random.next_long());
    }

    #[test]
    fn item_random_streams_are_independent() {
        let mut items = ItemEntities::default();
        items.entity_seed_checked = true;
        items.entity_seeder = Some(native_core::random::XoroshiroRandom::new(7));
        let stack = ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        items.spawn_from_entity(DVec3::ZERO, stack.clone());
        items.spawn_from_entity(DVec3::ZERO, stack);
        let mut second_before = items.entities[1].random.clone();
        let mut global_before = items.random.clone();
        let first_pitch = crate::extinguish_pitch(&mut items.entities[0].random);
        assert!(first_pitch >= 1.2 && first_pitch < 2.0);
        assert_eq!(
            items.entities[1].random.next_int(),
            second_before.next_int()
        );
        assert_eq!(items.random.next_int(), global_before.next_int());
    }

    #[test]
    fn item_uuid_comes_from_entity_random_and_missing_entropy_blocks_saving() {
        let stack = ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        let mut seeded = ItemEntities::default();
        seeded.entity_seed_checked = true;
        seeded.entity_seeder = Some(native_core::random::XoroshiroRandom::new(71));
        let mut seeder = seeded.entity_seeder.clone().unwrap();
        let (expected_uuid, _, _, _) = item_random_from_seed(seeder.next_long());
        seeded.spawn_from_entity(DVec3::ZERO, stack.clone());
        assert_eq!(
            seeded.entities[0].created.as_ref().unwrap().uuid,
            Some(expected_uuid)
        );

        let mut unavailable = ItemEntities::default();
        unavailable.entity_seed_checked = true;
        unavailable.spawn_from_entity(DVec3::ZERO, stack);
        assert_eq!(unavailable.entities[0].created.as_ref().unwrap().uuid, None);
        let pos = native_world::loaded::ChunkPos { x: 0, z: 0 };
        assert!(
            unavailable
                .validate_item_chunk_group(&BTreeSet::from([pos]), &Default::default())
                .is_err()
        );
    }

    #[test]
    fn reloaded_items_get_a_fresh_constructor_stream_and_keep_saved_state() {
        use native_world::loaded::ChunkPos;
        let mut items = ItemEntities::default();
        items.entity_seed_checked = true;
        items.entity_seeder = Some(native_core::random::XoroshiroRandom::new(7));
        let stack = ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        items.spawn_from_entity(DVec3::new(1.5, 5.0, 1.5), stack);
        let original = items.entities[0].clone();
        items.entities[0].item.age = 12;
        items.entities[0].random.next_int();
        let home = ChunkPos { x: 0, z: 0 };
        items.set_active_chunks(BTreeSet::from([home]));
        items.set_active_chunks(BTreeSet::new());
        let mut seeder = items.entity_seeder.clone().unwrap();
        let (_, bob, _, mut expected_random) = item_random_from_seed(seeder.next_long());
        items.set_active_chunks(BTreeSet::from([home]));
        let restored = &mut items.entities[0];
        assert_eq!(restored.id, 1);
        assert_eq!(restored.item.age, 0);
        assert_eq!(restored.item.bob_offset, bob);
        assert_eq!(restored.random.next_int(), expected_random.next_int());
        assert_eq!(restored.item.position, original.item.position);
        assert_eq!(restored.item.previous_position, original.item.position);
        assert_eq!(restored.velocity, original.velocity);
        assert_eq!(restored.lifetime, original.lifetime);
        assert_eq!(restored.pickup_delay, original.pickup_delay);
        assert_eq!(restored.created, original.created);
        assert_eq!(items.next_id, 2);
    }

    #[test]
    fn player_drop_records_only_known_throwers() {
        let mut items = ItemEntities::default();
        let stack = ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        let player = PlayerState {
            position: glam::Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: true,
            pose: PlayerPose::Standing,
        };
        let id = [1, 2, 3, 4];
        items.drop_player(
            stack.clone(),
            IVec3::ZERO,
            DVec3::ZERO,
            player,
            PlayerDropOrigin::Direct,
        );
        items.set_player_uuid(Some(id));
        items.drop_player(
            stack.clone(),
            IVec3::ZERO,
            DVec3::ZERO,
            player,
            PlayerDropOrigin::Direct,
        );
        items.drop_player(
            stack.clone(),
            IVec3::ZERO,
            DVec3::ZERO,
            player,
            PlayerDropOrigin::ContainerReturn,
        );
        items.drop_death(stack, IVec3::ZERO, DVec3::ZERO, player);
        let actual: Vec<_> = items
            .entities
            .iter()
            .map(|entity| entity.created.as_ref().unwrap().thrower)
            .collect();
        assert_eq!(
            actual,
            [
                CreatedThrower::Unknown,
                CreatedThrower::Known(Some(id)),
                CreatedThrower::Known(None),
                CreatedThrower::Known(None)
            ]
        );
    }

    #[test]
    fn created_item_group_requires_origin_destination_and_unique_identity() {
        use native_world::loaded::ChunkPos;
        use std::collections::BTreeMap;
        let home = ChunkPos { x: 0, z: 0 };
        let remote = ChunkPos { x: 1, z: 0 };
        let mut items = ItemEntities::default();
        let stack = ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        items.spawn_from_entity(DVec3::new(0.5, 70.0, 0.5), stack);
        let uuid = items.entities[0].created.as_ref().unwrap().uuid.unwrap();
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([home]), &BTreeMap::new())
                .is_ok()
        );
        assert!(
            items
                .validate_item_chunk_group(
                    &BTreeSet::from([home]),
                    &BTreeMap::from([(uuid, remote)])
                )
                .is_err()
        );
        items.set_player_uuid(Some(uuid));
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([home]), &BTreeMap::new())
                .is_err()
        );
        items.set_player_uuid(None);
        items.entities[0].item.position.x = 16.5;
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([home]), &BTreeMap::new())
                .is_err()
        );
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([remote]), &BTreeMap::new())
                .is_err()
        );
        let group = BTreeSet::from([home, remote]);
        assert!(
            items
                .validate_item_chunk_group(&group, &BTreeMap::new())
                .is_ok()
        );
        assert!(
            items.draft_created_items_in_chunks(&group).is_err(),
            "movement needs verified base state"
        );
    }

    #[test]
    fn certified_ticks_allow_created_draft_and_require_every_visited_chunk() {
        use native_world::loaded::ChunkPos;
        let home = ChunkPos { x: 0, z: 0 };
        let remote = ChunkPos { x: 1, z: 0 };
        let mut items = ItemEntities::default();
        let stack = ItemStack::new("minecraft:stone".into(), 2, 64, Tag::Compound(vec![])).unwrap();
        items.spawn_from_entity(DVec3::new(15.75, 5.0, 2.0), stack);
        items.entities[0].velocity = DVec3::new(1.0, -0.1, 0.0);
        items.tick_with_fluid_and_proof(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Spectator,
            &mut Arc::new(PlayerInventory::default()),
            |_| &[],
            |_| 0.6,
            |_| 0.0,
            |_| false,
            |_| native_world::fluid::FluidContacts::default(),
            false,
            |_, _, _| None,
            |_, _| true,
            |_| Some(false),
        );
        assert!(
            items
                .draft_created_items_in_chunks(&BTreeSet::from([home]))
                .is_err()
        );
        assert!(
            items
                .draft_created_items_in_chunks(&BTreeSet::from([remote]))
                .is_err()
        );
        let group = BTreeSet::from([home, remote]);
        let drafts = items.draft_created_items_in_chunks(&group).unwrap();
        let saved = crate::saved_item::read(&drafts[0].document().value)
            .unwrap()
            .unwrap();
        assert_eq!(saved.position, items.entities[0].item.position);
        assert_eq!(saved.velocity, items.entities[0].velocity);
        assert_eq!(saved.age, 1);
        assert_eq!(
            drafts[0].document().value.get("fall_distance"),
            Some(&Tag::Double(f64::from(0.14_f32)))
        );
        items.entities[0].velocity.x = -1.0;
        items.tick_with_fluid_and_proof(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Spectator,
            &mut Arc::new(PlayerInventory::default()),
            |_| &[],
            |_| 0.6,
            |_| 0.0,
            |_| false,
            |_| native_world::fluid::FluidContacts::default(),
            false,
            |_, _, _| None,
            |_, _| true,
            |_| Some(false),
        );
        assert!(items.entities[0].item.position.x < 16.0);
        assert!(
            items
                .draft_created_items_in_chunks(&BTreeSet::from([home]))
                .is_err(),
            "previously visited chunks remain required after returning"
        );
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([home]), &Default::default())
                .is_err()
        );
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([remote]), &Default::default())
                .is_err()
        );
        assert!(items.draft_created_items_in_chunks(&group).is_ok());
        items.entities[0].item.position.x += 0.25;
        assert!(
            items.draft_created_items_in_chunks(&group).is_err(),
            "untracked movement must be rejected"
        );
    }

    #[test]
    fn created_item_draft_tracks_fresh_state_and_rejects_after_tick_or_unknown_thrower() {
        use native_world::loaded::ChunkPos;
        let pos = ChunkPos { x: 0, z: 0 };
        let mut items = ItemEntities::default();
        let stack = ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        items.spawn_from_entity(DVec3::new(0.5, 70.0, 0.5), stack.clone());
        let identity = items.entities[0].created.clone().unwrap();
        let drafts = items.draft_created_items_in_chunk(pos).unwrap();
        let saved = crate::saved_item::read(&drafts[0].document().value)
            .unwrap()
            .unwrap();
        assert_eq!(saved.uuid, identity.uuid);
        assert_eq!(saved.position, identity.origin);
        assert_eq!(saved.pickup_delay, 10);
        assert_eq!(
            drafts[0].document().value.get("Rotation"),
            Some(&Tag::List {
                element_type: native_persistence::nbt::TagType::Float,
                values: vec![Tag::Float(identity.yaw), Tag::Float(0.0)],
            })
        );
        items.tick(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Survival,
            &mut Arc::new(PlayerInventory::default()),
            |_| &[],
            |_| 0.6,
        );
        assert!(items.draft_created_items_in_chunk(pos).is_err());
        items.set_active_chunks(BTreeSet::new());
        items.set_active_chunks(BTreeSet::from([pos]));
        assert_eq!(items.entities[0].item.age, 0);
        assert!(items.draft_created_items_in_chunk(pos).is_err());
        let mut unknown = ItemEntities::default();
        let player = PlayerState {
            position: glam::Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: true,
            pose: PlayerPose::Standing,
        };
        unknown.drop_player(
            stack,
            IVec3::ZERO,
            DVec3::ZERO,
            player,
            PlayerDropOrigin::Direct,
        );
        assert!(unknown.draft_created_items_in_chunk(pos).is_err());
    }

    #[test]
    fn created_item_fall_distance_uses_java_float_displacement_and_ground_reset() {
        let mut items = ItemEntities::default();
        let stack = ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        items.spawn_from_entity(DVec3::new(0.5, 1.0, 0.5), stack);
        items.entities[0].velocity = DVec3::new(0.0, -0.1, 0.0);
        let mut inventory = Arc::new(PlayerInventory::default());
        let shape = |pos: IVec3| {
            if pos.y < 0 {
                native_world::collision::unknown_barrier()
            } else {
                &[]
            }
        };
        items.tick(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            shape,
            |_| 0.6,
        );
        assert_eq!(
            items.entities[0].created.as_ref().unwrap().fall_distance,
            f64::from(0.14_f32)
        );
        for _ in 0..30 {
            items.tick(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Survival,
                &mut inventory,
                shape,
                |_| 0.6,
            );
            if items.entities[0].on_ground {
                break;
            }
        }
        assert!(items.entities[0].on_ground);
        assert_eq!(
            items.entities[0].created.as_ref().unwrap().fall_distance,
            0.0
        );
    }

    #[test]
    fn verified_created_pickup_and_expiry_require_complete_path_groups() {
        use native_world::loaded::ChunkPos;
        let left = ChunkPos { x: 0, z: 0 };
        let right = ChunkPos { x: 1, z: 0 };
        let group = BTreeSet::from([left, right]);
        let stack = ItemStack::new("minecraft:stone".into(), 3, 64, Tag::Compound(vec![])).unwrap();
        let mut items = ItemEntities::default();
        items.spawn_from_entity(DVec3::new(15.75, 5.0, 2.0), stack.clone());
        let uuid = items.entities[0].created.as_ref().unwrap().uuid.unwrap();
        items.entities[0].velocity = DVec3::new(1.0, 0.0, 0.0);
        items.entities[0].pickup_delay = 0;
        let mut inventory = Arc::new(PlayerInventory::default());
        items.tick_with_fluid_and_proof(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Spectator,
            &mut inventory,
            |_| &[],
            |_| 0.6,
            |_| 0.0,
            |_| false,
            |_| native_world::fluid::FluidContacts::default(),
            false,
            |_, _, _| None,
            |_, _| true,
            |_| Some(false),
        );
        let player = items.entities[0].item.position;
        items.tick_with_fluid_and_proof(
            IVec3::ZERO,
            player,
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
            |_| 0.0,
            |_| false,
            |_| native_world::fluid::FluidContacts::default(),
            false,
            |_, _, _| None,
            |_, _| true,
            |_| Some(false),
        );
        assert!(items.entities.is_empty());
        assert_eq!(
            items.affected_item_chunks(&Default::default()).unwrap(),
            group
        );
        assert_eq!(
            inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            3
        );
        assert_eq!(
            items.created_removal_effects(&group),
            vec![(uuid, CreatedRemovalCause::Pickup)]
        );
        assert_eq!(
            items.created_pickup_effects(&group),
            vec![CreatedItemPickup {
                uuid,
                original_stack: stack.clone(),
                transferred: 3,
                discarded: 0,
                pickup_succeeded: true,
            }]
        );
        assert!(
            items
                .validate_removed_item_chunk_group(&BTreeSet::from([left]), &Default::default())
                .is_err()
        );
        assert!(
            items
                .validate_removed_item_chunk_group(&BTreeSet::from([right]), &Default::default())
                .is_err()
        );
        assert!(
            items
                .validate_removed_item_chunk_group(&group, &Default::default())
                .is_ok()
        );
        assert!(
            items
                .validate_item_chunk_ownership(left, &Default::default())
                .is_err(),
            "single chunk drafts must not omit coordinated removals"
        );
        assert!(
            items
                .validate_removed_item_chunk_group(
                    &group,
                    &std::collections::BTreeMap::from([(uuid, left)])
                )
                .is_err()
        );
        items.set_player_uuid(Some(uuid));
        assert!(
            items
                .validate_removed_item_chunk_group(&group, &Default::default())
                .is_err()
        );

        let mut expired = ItemEntities::default();
        expired.spawn_from_entity(DVec3::new(0.5, 5.0, 0.5), stack);
        let expired_uuid = expired.entities[0].created.as_ref().unwrap().uuid.unwrap();
        expired.entities[0].lifetime = 5999;
        expired.tick_with_fluid_and_proof(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Spectator,
            &mut inventory,
            |_| &[],
            |_| 0.6,
            |_| 0.0,
            |_| false,
            |_| native_world::fluid::FluidContacts::default(),
            false,
            |_, _, _| None,
            |_, _| true,
            |_| Some(false),
        );
        assert!(expired.entities.is_empty());
        assert_eq!(
            expired.affected_item_chunks(&Default::default()).unwrap(),
            BTreeSet::from([left])
        );
        assert_eq!(
            expired.created_removal_effects(&BTreeSet::from([left])),
            vec![(expired_uuid, CreatedRemovalCause::Expired)]
        );
        assert!(
            expired
                .validate_removed_item_chunk_group(&BTreeSet::from([left]), &Default::default())
                .is_ok()
        );
    }

    #[test]
    fn created_item_partial_pickup_records_each_transfer_and_remaining_stack() {
        use crate::inventory::PlayerSlot;
        use native_world::loaded::ChunkPos;
        let mut inventory = PlayerInventory::default();
        assert!(inventory.set_slot(
            PlayerSlot::Storage(0),
            Some(ItemStack::new("minecraft:stone".into(), 63, 64, Tag::Compound(vec![])).unwrap())
        ));
        for slot in 1..36 {
            assert!(inventory.set_slot(
                PlayerSlot::Storage(slot),
                Some(
                    ItemStack::new("minecraft:dirt".into(), 64, 64, Tag::Compound(vec![])).unwrap()
                )
            ));
        }
        let mut inventory = Arc::new(inventory);
        let mut items = ItemEntities::default();
        let position = DVec3::new(1.5, 2.0, 3.5);
        let stack = ItemStack::new("minecraft:stone".into(), 3, 64, Tag::Compound(vec![])).unwrap();
        items.spawn_from_entity(position, stack.clone());
        items.entities[0].velocity = DVec3::ZERO;
        items.entities[0].no_gravity = true;
        items.entities[0].pickup_delay = 0;
        let uuid = items.entities[0].created.as_ref().unwrap().uuid.unwrap();
        let group = BTreeSet::from([ChunkPos { x: 0, z: 0 }]);
        let tick = |items: &mut ItemEntities, inventory: &mut Arc<PlayerInventory>| {
            items.tick_with_fluid_and_proof(
                IVec3::ZERO,
                position,
                PlayerPose::Standing,
                GameMode::Survival,
                inventory,
                |_| &[],
                |_| 0.6,
                |_| 0.0,
                |_| false,
                |_| native_world::fluid::FluidContacts::default(),
                false,
                |_, _, _| None,
                |_, _| true,
                |_| Some(false),
            );
        };
        tick(&mut items, &mut inventory);
        assert_eq!(items.entities[0].item.stack.count(), 2);
        assert!(items.take_pickup_sounds().is_empty());
        assert_eq!(inventory.slot(PlayerSlot::Storage(0)).unwrap().count(), 64);
        assert_eq!(
            items.created_pickup_effects(&group),
            vec![CreatedItemPickup {
                uuid,
                original_stack: stack.clone(),
                transferred: 1,
                discarded: 0,
                pickup_succeeded: false,
            }]
        );
        let draft = items.draft_created_items_in_chunks(&group).unwrap();
        assert_eq!(
            crate::saved_item::read(&draft[0].document().value)
                .unwrap()
                .unwrap()
                .stack
                .count(),
            2
        );
        tick(&mut items, &mut inventory);
        assert_eq!(
            items.created_pickup_effects(&group).len(),
            1,
            "no capacity produces no transfer"
        );
        assert!(Arc::make_mut(&mut inventory).set_slot(PlayerSlot::Storage(1), None));
        tick(&mut items, &mut inventory);
        assert!(items.entities.is_empty());
        assert_eq!(items.take_pickup_sounds(), vec![position]);
        assert!(items.take_pickup_sounds().is_empty());
        assert_eq!(
            items.created_pickup_effects(&group),
            vec![
                CreatedItemPickup {
                    uuid,
                    original_stack: stack.clone(),
                    transferred: 1,
                    discarded: 0,
                    pickup_succeeded: false,
                },
                CreatedItemPickup {
                    uuid,
                    original_stack: stack.with_count(2).unwrap(),
                    transferred: 2,
                    discarded: 0,
                    pickup_succeeded: true,
                }
            ]
        );
        assert_eq!(
            items.created_removal_effects(&group),
            vec![(uuid, CreatedRemovalCause::Pickup)]
        );
        assert!(
            items
                .validate_removed_item_chunk_group(&group, &Default::default())
                .is_ok()
        );
    }

    #[test]
    fn creative_pickup_records_inserted_and_discarded_counts() {
        use crate::inventory::PlayerSlot;
        use native_world::loaded::ChunkPos;
        for inserted in [0, 1] {
            let mut inventory = PlayerInventory::default();
            for slot in 0..36 {
                let (id, count) = if slot == 0 {
                    ("minecraft:stone", 64 - inserted)
                } else {
                    ("minecraft:dirt", 64)
                };
                assert!(inventory.set_slot(
                    PlayerSlot::Storage(slot),
                    Some(ItemStack::new(id.into(), count, 64, Tag::Compound(vec![])).unwrap())
                ));
            }
            let mut inventory = Arc::new(inventory);
            let mut items = ItemEntities::default();
            let position = DVec3::new(1.5, 2.0, 3.5);
            let stack =
                ItemStack::new("minecraft:stone".into(), 3, 64, Tag::Compound(vec![])).unwrap();
            items.spawn_from_entity(position, stack.clone());
            items.entities[0].velocity = DVec3::ZERO;
            items.entities[0].no_gravity = true;
            items.entities[0].pickup_delay = 0;
            let uuid = items.entities[0].created.as_ref().unwrap().uuid.unwrap();
            items.tick_with_fluid_and_proof(
                IVec3::ZERO,
                position,
                PlayerPose::Standing,
                GameMode::Creative,
                &mut inventory,
                |_| &[],
                |_| 0.6,
                |_| 0.0,
                |_| false,
                |_| native_world::fluid::FluidContacts::default(),
                false,
                |_, _, _| None,
                |_, _| true,
                |_| Some(false),
            );
            let group = BTreeSet::from([ChunkPos { x: 0, z: 0 }]);
            assert!(items.entities.is_empty());
            assert_eq!(items.take_pickup_sounds(), vec![position]);
            assert_eq!(inventory.slot(PlayerSlot::Storage(0)).unwrap().count(), 64);
            assert_eq!(
                items.created_pickup_effects(&group),
                vec![CreatedItemPickup {
                    uuid,
                    original_stack: stack,
                    transferred: inserted,
                    discarded: 3 - inserted,
                    pickup_succeeded: true,
                }]
            );
            assert_eq!(
                items.created_removal_effects(&group),
                vec![(uuid, CreatedRemovalCause::Pickup)]
            );
        }
    }

    #[test]
    fn silent_saved_item_is_collected_without_pickup_sound() {
        let mut items = ItemEntities::default();
        let position = DVec3::new(1.5, 2.0, 3.5);
        let stack = ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        items.spawn_from_entity(position, stack);
        let entity = &mut items.entities[0];
        entity.velocity = DVec3::ZERO;
        entity.no_gravity = true;
        entity.pickup_delay = 0;
        entity._saved_source = Some(Arc::new(Tag::Compound(vec![(
            "Silent".into(),
            Tag::Byte(1),
        )])));
        let mut inventory = Arc::new(PlayerInventory::default());
        items.tick_with_fluid_and_proof(
            IVec3::ZERO,
            position,
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
            |_| 0.0,
            |_| false,
            |_| native_world::fluid::FluidContacts::default(),
            false,
            |_, _, _| None,
            |_, _| true,
            |_| Some(false),
        );
        assert!(items.entities.is_empty());
        assert!(items.take_pickup_sounds().is_empty());
        assert_eq!(
            inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn expired_created_item_blocks_both_affected_chunks() {
        use native_world::loaded::ChunkPos;
        let mut items = ItemEntities::default();
        let stack = ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        items.spawn_from_entity(DVec3::new(0.5, 70.0, 0.5), stack);
        items.entities[0].item.position.x = 32.5;
        items.entities[0].lifetime = 5999;
        items.tick(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Survival,
            &mut Arc::new(PlayerInventory::default()),
            |_| &[],
            |_| 0.6,
        );
        assert!(items.entities.is_empty());
        for x in [0, 2] {
            assert!(
                items
                    .validate_removed_item_chunk_group(
                        &BTreeSet::from([ChunkPos { x, z: 0 }]),
                        &Default::default()
                    )
                    .is_err()
            );
        }
        assert!(
            items
                .validate_removed_item_chunk_group(
                    &BTreeSet::from([ChunkPos { x: 1, z: 0 }]),
                    &Default::default()
                )
                .is_ok()
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn vanilla_damage_prototypes_and_active_tags_resolve_together() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = native_resources::packs::ResourceStack::open(&game).unwrap();
        let items = source.vanilla_item_registry().unwrap();
        let tags = native_resources::enchantments::OxygenEnchantments::load(&mut source).unwrap();
        for (id, expected) in [
            ("minecraft:dirt", Some(false)),
            ("minecraft:netherite_ingot", Some(true)),
            ("minecraft:nether_star", Some(false)),
            ("example:unknown", None),
        ] {
            let stack = ItemStack::new(id.into(), 1, 64, Tag::Compound(vec![])).unwrap();
            assert_eq!(
                damage_resistant_in_world(&stack, &items, &tags, "minecraft:lava").unwrap(),
                expected,
                "{id}"
            );
        }
        let star =
            ItemStack::new("minecraft:nether_star".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        assert_eq!(
            damage_resistant_in_world(&star, &items, &tags, "minecraft:explosion").unwrap(),
            Some(true)
        );
        struct ReplacedTags;
        impl ResourceProvider for ReplacedTags {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(
                &mut self,
                folder: &str,
            ) -> anyhow::Result<Vec<native_resources::packs::ResourceId>> {
                match folder {
                    "tags/damage_type" => Ok(vec![native_resources::packs::ResourceId::parse(
                        "minecraft:is_explosion",
                    )?]),
                    "damage_type" => Ok(vec![
                        native_resources::packs::ResourceId::parse("minecraft:lava")?,
                        native_resources::packs::ResourceId::parse("minecraft:explosion")?,
                    ]),
                    _ => Ok(vec![]),
                }
            }
            fn read_data_stack(&mut self, path: &str) -> anyhow::Result<Vec<Vec<u8>>> {
                Ok(
                    if path == "data/minecraft/tags/damage_type/is_explosion.json" {
                        vec![br#"{"replace":true,"values":["minecraft:lava"]}"#.to_vec()]
                    } else {
                        vec![]
                    },
                )
            }
        }
        let replaced =
            native_resources::enchantments::OxygenEnchantments::load(&mut ReplacedTags).unwrap();
        assert_eq!(
            damage_resistant_in_world(&star, &items, &replaced, "minecraft:lava").unwrap(),
            Some(true)
        );
        assert_eq!(
            damage_resistant_in_world(&star, &items, &replaced, "minecraft:explosion").unwrap(),
            Some(false)
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn lava_contact_uses_vanilla_resistance_and_saved_health() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = native_resources::packs::ResourceStack::open(&game).unwrap();
        let rules = (
            Arc::new(source.vanilla_item_registry().unwrap()),
            Arc::new(
                native_resources::enchantments::OxygenEnchantments::load(&mut source).unwrap(),
            ),
        );
        let make = |id: &str| ItemEntity {
            item: DroppedItem {
                entity_id: 0,
                item_age: 0,
                position: DVec3::ZERO,
                previous_position: DVec3::ZERO,
                stack: ItemStack::new(id.into(), 1, 64, Tag::Compound(vec![])).unwrap(),
                age: 0,
                bob_offset: 0.0,
            },
            velocity: DVec3::ZERO,
            on_ground: false,
            ground_without_support: false,
            pickup_delay: 0,
            lifetime: 0,
            no_gravity: false,
            owner: None,
            _saved_source: None,
            created: None,
            id: 0,
            section_sequence: 0,
            random: LegacyRandom::new(0),
            _damage_state: ItemDamageState::default(),
        };
        let mut stone = make("minecraft:stone");
        assert!(!apply_lava_contact(&mut stone, &rules).unwrap());
        assert_eq!(stone._damage_state.health, 1);
        assert_eq!(stone._damage_state.fire_ticks, 300);
        assert!(apply_lava_contact(&mut stone, &rules).unwrap());
        let mut netherite = make("minecraft:netherite_ingot");
        assert!(!apply_lava_contact(&mut netherite, &rules).unwrap());
        assert_eq!(netherite._damage_state, ItemDamageState::default());
        let mut invulnerable = make("minecraft:stone");
        invulnerable._damage_state.invulnerable = true;
        assert!(!apply_lava_contact(&mut invulnerable, &rules).unwrap());
        assert_eq!(invulnerable._damage_state.health, 5);
        assert_eq!(invulnerable._damage_state.fire_ticks, 300);
        let mut star = make("minecraft:nether_star");
        assert!(!apply_lava_contact(&mut star, &rules).unwrap());
        assert_eq!(star._damage_state.health, 1);
        assert_eq!(star._damage_state.fire_ticks, 300);
        assert!(apply_lava_contact(&mut star, &rules).unwrap());

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
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let file = temp.path().join("ItemLavaOracle.java");
        std::fs::write(&file, include_str!("../tests/java/ItemLavaOracle.java")).unwrap();
        let run = |command: &mut std::process::Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "lava Java oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(std::process::Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&file));
        let result = temp.path().join("lava.json");
        let fire_result = temp.path().join("fire.json");
        let rain_result = temp.path().join("rain.json");
        run(std::process::Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("ItemLavaOracle")
            .arg(&result)
            .arg(&fire_result)
            .arg(&rain_result));
        let oracle: serde_json::Value =
            serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        for mode in 0..4 {
            let mut item = make(match mode {
                1 => "minecraft:netherite_ingot",
                3 => "minecraft:nether_star",
                _ => "minecraft:stone",
            });
            item._damage_state.invulnerable = mode == 2;
            for row in oracle[mode].as_array().unwrap() {
                let removed = apply_lava_contact(&mut item, &rules).unwrap();
                assert_eq!(removed, row["removed"].as_bool().unwrap(), "mode {mode}");
                assert_eq!(
                    i64::from(item._damage_state.health),
                    row["health"].as_i64().unwrap(),
                    "mode {mode}"
                );
                assert_eq!(
                    i64::from(item._damage_state.fire_ticks),
                    row["fire"].as_i64().unwrap(),
                    "mode {mode}"
                );
            }
        }
        let rain_oracle: Vec<bool> =
            serde_json::from_slice(&std::fs::read(rain_result).unwrap()).unwrap();
        let mut index = 0;
        for position in [
            DVec3::new(0.5, 2.1, 0.5),
            DVec3::new(-0.25, 2.8, -1.1),
            DVec3::new(16.0, -0.25, -16.0),
            DVec3::new(0.0, -1.0, 0.0),
        ] {
            let feet = position.floor().as_ivec3();
            let top = IVec3::new(feet.x, (position.y + 0.25).floor() as i32, feet.z);
            for mask in 0..4 {
                let actual = item_in_rain(position, |at| {
                    Some((mask & 1 != 0 && at == feet) || (mask & 2 != 0 && at == top))
                });
                assert_eq!(
                    actual,
                    Some(rain_oracle[index]),
                    "position {position:?}, mask {mask}"
                );
                index += 1;
            }
        }
        let fire_oracle: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fire_result).unwrap()).unwrap();
        for mode in 0..4 {
            let mut item = make(match mode {
                1 => "minecraft:netherite_ingot",
                3 => "minecraft:nether_star",
                _ => "minecraft:stone",
            });
            item._damage_state.invulnerable = mode == 2;
            item._damage_state.fire_ticks = 41;
            for row in fire_oracle[mode].as_array().unwrap() {
                let removed = apply_fire_tick(&mut item, &rules, false).unwrap();
                assert_eq!(
                    removed,
                    row["removed"].as_bool().unwrap(),
                    "fire mode {mode}"
                );
                assert_eq!(
                    i64::from(item._damage_state.health),
                    row["health"].as_i64().unwrap(),
                    "fire mode {mode}"
                );
                assert_eq!(
                    i64::from(item._damage_state.fire_ticks),
                    row["fire"].as_i64().unwrap(),
                    "fire mode {mode}"
                );
            }
        }
    }

    #[test]
    fn damage_resistance_resolves_patches_tags_and_unknown_prototypes() {
        let stack =
            |components| ItemStack::new("minecraft:dirt".into(), 1, 64, components).unwrap();
        let empty = stack(Tag::Compound(vec![]));
        let tags =
            |tag: &str, damage: &str| Ok(tag == "example:fire" && damage == "minecraft:lava");
        assert_eq!(
            damage_resistant(
                &empty,
                DamageResistancePrototype::Unknown,
                "lava",
                tags,
                |_| Ok(true)
            )
            .unwrap(),
            None
        );
        assert_eq!(
            damage_resistant(
                &empty,
                DamageResistancePrototype::Absent,
                "lava",
                tags,
                |_| Ok(true)
            )
            .unwrap(),
            Some(false)
        );
        let prototype = Tag::Compound(vec![("types".into(), Tag::String("#example:fire".into()))]);
        assert_eq!(
            damage_resistant(
                &empty,
                DamageResistancePrototype::Component(&prototype),
                "lava",
                tags,
                |_| Ok(true)
            )
            .unwrap(),
            Some(true)
        );
        let patch = stack(Tag::Compound(vec![(
            "minecraft:damage_resistant".into(),
            Tag::Compound(vec![(
                "types".into(),
                Tag::List {
                    element_type: native_persistence::nbt::TagType::String,
                    values: vec![
                        Tag::String("minecraft:in_fire".into()),
                        Tag::String("minecraft:lava".into()),
                    ],
                },
            )]),
        )]));
        assert_eq!(
            damage_resistant(
                &patch,
                DamageResistancePrototype::Unknown,
                "lava",
                tags,
                |_| Ok(true)
            )
            .unwrap(),
            Some(true)
        );
        assert_eq!(
            damage_resistant(
                &patch,
                DamageResistancePrototype::Unknown,
                "minecraft:fall",
                tags,
                |_| Ok(true)
            )
            .unwrap(),
            Some(false)
        );
        let removed = stack(Tag::Compound(vec![(
            "!minecraft:damage_resistant".into(),
            Tag::Compound(vec![]),
        )]));
        assert_eq!(
            damage_resistant(
                &removed,
                DamageResistancePrototype::Component(&prototype),
                "lava",
                tags,
                |_| Ok(true)
            )
            .unwrap(),
            Some(false)
        );
        let invalid = stack(Tag::Compound(vec![(
            "minecraft:damage_resistant".into(),
            Tag::Int(1),
        )]));
        assert!(
            damage_resistant(
                &invalid,
                DamageResistancePrototype::Absent,
                "lava",
                tags,
                |_| Ok(true)
            )
            .is_err()
        );
    }

    #[test]
    fn damage_resistance_uses_replaced_world_damage_tags() {
        struct Packs;
        impl ResourceProvider for Packs {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(
                &mut self,
                folder: &str,
            ) -> anyhow::Result<Vec<native_resources::packs::ResourceId>> {
                if folder == "tags/damage_type" {
                    Ok(vec![native_resources::packs::ResourceId::parse(
                        "example:resistant",
                    )?])
                } else if folder == "damage_type" {
                    Ok(vec![
                        native_resources::packs::ResourceId::parse("minecraft:lava")?,
                        native_resources::packs::ResourceId::parse("minecraft:fall")?,
                    ])
                } else {
                    Ok(vec![])
                }
            }
            fn read_data_stack(&mut self, path: &str) -> anyhow::Result<Vec<Vec<u8>>> {
                Ok(if path == "data/example/tags/damage_type/resistant.json" {
                    vec![
                        br#"{"values":["minecraft:lava"]}"#.to_vec(),
                        br#"{"replace":true,"values":["minecraft:fall"]}"#.to_vec(),
                    ]
                } else {
                    vec![]
                })
            }
        }
        let registry =
            native_resources::enchantments::OxygenEnchantments::load(&mut Packs).unwrap();
        let item = ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        let prototype = Tag::Compound(vec![(
            "types".into(),
            Tag::String("#example:resistant".into()),
        )]);
        let query = |damage| {
            damage_resistant(
                &item,
                DamageResistancePrototype::Component(&prototype),
                damage,
                |tag, damage| registry.damage_type_has_tag(tag, damage),
                |damage| registry.has_damage_type(damage),
            )
            .unwrap()
        };
        assert_eq!(query("lava"), Some(false));
        assert_eq!(query("fall"), Some(true));
    }

    #[test]
    fn spawned_saved_item_retains_damage_state() {
        let mut items = ItemEntities::default();
        items.spawn_saved(crate::saved_item::SavedItemState {
            source: Arc::new(Tag::Compound(vec![])),
            uuid: None,
            position: DVec3::ZERO,
            velocity: DVec3::ZERO,
            on_ground: false,
            no_gravity: false,
            age: 0,
            pickup_delay: 0,
            health: 3,
            fire_ticks: 25,
            invulnerable: true,
            invulnerable_time: 8,
            owner: None,
            thrower: None,
            stack: ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![])).unwrap(),
        });
        assert_eq!(
            items.entities[0]._damage_state,
            ItemDamageState {
                health: 3,
                fire_ticks: 25,
                invulnerable: true,
                invulnerable_time: 8,
            }
        );
    }

    fn test_saved_item(uuid: [i32; 4], count: u32) -> crate::saved_item::SavedItemState {
        use native_persistence::nbt::TagType;
        let entity = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(uuid.to_vec())),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: vec![Tag::Double(1.5), Tag::Double(2.0), Tag::Double(3.5)],
                },
            ),
            ("NoGravity".into(), Tag::Byte(1)),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(count as i32)),
                ]),
            ),
        ]);
        let mut saved = crate::saved_item::read(&entity).unwrap().unwrap();
        saved.stack =
            ItemStack::new("minecraft:stone".into(), count, 64, Tag::Compound(vec![])).unwrap();
        saved
    }

    #[test]
    fn removed_item_requires_source_and_final_chunks() {
        use native_world::loaded::ChunkPos;
        let uuid = [1, 2, 3, 4];
        let home = ChunkPos { x: 0, z: 0 };
        let destination = ChunkPos { x: 1, z: 0 };
        let mut items = ItemEntities::default();
        items.spawn_saved(test_saved_item(uuid, 3));
        items.entities[0].item.position.x = 16.5;
        let mut inventory = Arc::new(PlayerInventory::default());
        items.tick(
            IVec3::ZERO,
            DVec3::new(16.5, 2.0, 3.5),
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
        );
        assert!(items.entities.is_empty());
        assert!(items.draft_removed_item(uuid).unwrap().is_some());
        let sources = std::collections::BTreeMap::from([(uuid, home)]);
        assert!(
            items
                .validate_removed_item_chunk_group(&BTreeSet::from([home]), &sources)
                .is_err()
        );
        assert!(
            items
                .validate_removed_item_chunk_group(&BTreeSet::from([destination]), &sources)
                .is_err()
        );
        assert!(
            items
                .validate_removed_item_chunk_group(&BTreeSet::from([home, destination]), &sources)
                .is_ok()
        );
        items.spawn_saved(test_saved_item(uuid, 3));
        assert!(
            items
                .validate_removed_item_chunk_group(&BTreeSet::from([home, destination]), &sources)
                .is_err()
        );
    }

    #[test]
    fn merged_saved_item_removal_requires_both_sources() {
        use native_world::loaded::ChunkPos;
        let uuid = [1, 2, 3, 4];
        let other_uuid = [5, 6, 7, 8];
        let mut items = ItemEntities::default();
        items.spawn_saved(test_saved_item(uuid, 1));
        items.spawn_saved(test_saved_item(other_uuid, 2));
        let mut inventory = Arc::new(PlayerInventory::default());
        for _ in 0..40 {
            items.tick(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Survival,
                &mut inventory,
                |_| &[],
                |_| 0.6,
            );
        }
        assert_eq!(items.entities.len(), 1);
        assert!(items.draft_removed_item(uuid).unwrap().is_some());
        let home = ChunkPos { x: 0, z: 0 };
        let sources = std::collections::BTreeMap::from([(uuid, home), (other_uuid, home)]);
        assert!(
            items
                .validate_removed_item_chunk_group(&BTreeSet::from([home]), &sources)
                .is_ok()
        );
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([home]), &sources)
                .is_ok()
        );
    }

    #[test]
    fn expired_saved_item_has_a_valid_removal_draft() {
        use native_world::loaded::ChunkPos;
        let uuid = [1, 2, 3, 4];
        let mut saved = test_saved_item(uuid, 1);
        saved.age = 5999;
        let mut items = ItemEntities::default();
        items.spawn_saved(saved);
        let mut inventory = Arc::new(PlayerInventory::default());
        items.tick(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
        );
        assert!(items.entities.is_empty());
        let draft = items.draft_removed_item(uuid).unwrap().unwrap();
        assert_eq!(
            crate::saved_item::read(&draft.document().value)
                .unwrap()
                .unwrap()
                .age,
            6000
        );
        let home = ChunkPos { x: 0, z: 0 };
        assert!(
            items
                .validate_removed_item_chunk_group(
                    &BTreeSet::from([home]),
                    &std::collections::BTreeMap::from([(uuid, home)])
                )
                .is_ok()
        );
    }

    #[test]
    fn partial_saved_merge_can_save_when_all_chunks_are_present() {
        use native_world::loaded::ChunkPos;
        let a = [1, 2, 3, 4];
        let b = [5, 6, 7, 8];
        let mut items = ItemEntities::default();
        for (uuid, count) in [(a, 60), (b, 20)] {
            let mut saved = test_saved_item(uuid, count);
            let components = Tag::Compound(vec![("minecraft:max_stack_size".into(), Tag::Int(99))]);
            let Tag::Compound(entity) = Arc::make_mut(&mut saved.source) else {
                unreachable!()
            };
            let (_, Tag::Compound(stack)) = entity
                .iter_mut()
                .find(|(name, _)| name.matches("Item"))
                .unwrap()
            else {
                unreachable!()
            };
            stack.push(("components".into(), components.clone()));
            saved.stack = ItemStack::new("minecraft:stone".into(), count, 99, components).unwrap();
            items.spawn_saved(saved);
        }
        let mut inventory = Arc::new(PlayerInventory::default());
        for _ in 0..40 {
            items.tick(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Survival,
                &mut inventory,
                |_| &[],
                |_| 0.6,
            );
        }
        assert_eq!(items.entities.len(), 2);
        assert!(!items.uncoordinated_saved_merge);
        assert!(!items.saved_merges.is_empty());
        let home = ChunkPos { x: 0, z: 0 };
        let owners = std::collections::BTreeMap::from([(a, home), (b, home)]);
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([home]), &owners)
                .is_ok()
        );
        assert!(items.validate_item_chunk_ownership(home, &owners).is_ok());
    }

    #[test]
    fn cross_chunk_merge_requires_sources_merge_positions_and_final_location() {
        use native_world::loaded::ChunkPos;
        let a = [1, 2, 3, 4];
        let b = [5, 6, 7, 8];
        let mut items = ItemEntities::default();
        for (uuid, count, x) in [(a, 1, 15.8), (b, 2, 16.2)] {
            let mut saved = test_saved_item(uuid, count);
            saved.position.x = x;
            let Tag::Compound(fields) = Arc::make_mut(&mut saved.source) else {
                unreachable!()
            };
            let (_, Tag::List { values, .. }) = fields
                .iter_mut()
                .find(|(name, _)| name.matches("Pos"))
                .unwrap()
            else {
                unreachable!()
            };
            values[0] = Tag::Double(x);
            items.spawn_saved(saved);
        }
        let mut inventory = Arc::new(PlayerInventory::default());
        for _ in 0..40 {
            items.tick(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Survival,
                &mut inventory,
                |_| &[],
                |_| 0.6,
            );
        }
        assert_eq!(items.entities.len(), 1);
        let left = ChunkPos { x: 0, z: 0 };
        let right = ChunkPos { x: 1, z: 0 };
        let farther = ChunkPos { x: 2, z: 0 };
        let owners = std::collections::BTreeMap::from([(a, left), (b, right)]);
        assert_eq!(
            items.affected_item_chunks(&owners).unwrap(),
            BTreeSet::from([left, right])
        );
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([left]), &owners)
                .is_err()
        );
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([right]), &owners)
                .is_err()
        );
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([left, right]), &owners)
                .is_ok()
        );
        items.entities[0].item.position.x = 32.2;
        assert_eq!(
            items.affected_item_chunks(&owners).unwrap(),
            BTreeSet::from([left, right, farther])
        );
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([left, right]), &owners)
                .is_err()
        );
        assert!(
            items
                .validate_item_chunk_group(&BTreeSet::from([left, right, farther]), &owners)
                .is_ok()
        );
    }

    #[test]
    fn partial_merge_of_created_items_blocks_independent_chunk_drafts() {
        use native_world::loaded::ChunkPos;
        let mut items = ItemEntities::default();
        for (x, count) in [(15.8, 60), (16.2, 20)] {
            let components = Tag::Compound(vec![("minecraft:max_stack_size".into(), Tag::Int(99))]);
            let stack = ItemStack::new("minecraft:stone".into(), count, 99, components).unwrap();
            items.spawn_from_entity(DVec3::new(x, 5.0, 2.0), stack);
            let entity = items.entities.last_mut().unwrap();
            entity.velocity = DVec3::ZERO;
            entity.no_gravity = true;
            entity.pickup_delay = 0;
        }
        let mut inventory = Arc::new(PlayerInventory::default());
        for _ in 0..40 {
            items.tick_with_fluid_and_proof(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Spectator,
                &mut inventory,
                |_| &[],
                |_| 0.6,
                |_| 0.0,
                |_| false,
                |_| native_world::fluid::FluidContacts::default(),
                false,
                |_, _, _| None,
                |_, _| true,
                |_| Some(false),
            );
        }
        assert_eq!(items.entities.len(), 2, "partial merge keeps both entities");
        assert!(items.uncoordinated_saved_merge);
        assert!(items.affected_item_chunks(&Default::default()).is_err());
        for pos in [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 1, z: 0 }] {
            assert!(
                items
                    .validate_item_chunk_group(&BTreeSet::from([pos]), &Default::default())
                    .is_err()
            );
        }
    }

    #[test]
    fn merge_with_a_new_unsaved_item_remains_blocked() {
        use native_world::loaded::ChunkPos;
        let uuid = [1, 2, 3, 4];
        let mut items = ItemEntities::default();
        items.spawn_saved(test_saved_item(uuid, 1));
        items.spawn_from_entity(
            DVec3::new(1.5, 2.0, 3.5),
            ItemStack::new("minecraft:stone".into(), 2, 64, Tag::Compound(vec![])).unwrap(),
        );
        items.entities[1].velocity = DVec3::ZERO;
        items.entities[1].no_gravity = true;
        let mut inventory = Arc::new(PlayerInventory::default());
        for _ in 0..40 {
            items.tick(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Survival,
                &mut inventory,
                |_| &[],
                |_| 0.6,
            );
        }
        assert!(items.uncoordinated_saved_merge);
        let home = ChunkPos { x: 0, z: 0 };
        assert!(
            items
                .validate_item_chunk_group(
                    &BTreeSet::from([home]),
                    &std::collections::BTreeMap::from([(uuid, home)])
                )
                .is_err()
        );
    }

    #[test]
    fn saved_item_chunk_ownership_rejects_new_moved_and_duplicate_entities() {
        let mut items = ItemEntities::default();
        let pos = native_world::loaded::ChunkPos { x: -1, z: 0 };
        let other = native_world::loaded::ChunkPos { x: 0, z: 0 };
        let uuid = [1, 2, 3, 4];
        items.spawn_saved(crate::saved_item::SavedItemState {
            source: Arc::new(Tag::Compound(vec![(
                "UUID".into(),
                Tag::IntArray(uuid.to_vec()),
            )])),
            uuid: Some(uuid),
            position: DVec3::new(-0.5, 2.0, 3.0),
            velocity: DVec3::ZERO,
            on_ground: false,
            no_gravity: false,
            age: 0,
            pickup_delay: 0,
            health: 5,
            fire_ticks: 0,
            invulnerable: false,
            invulnerable_time: 0,
            owner: None,
            thrower: None,
            stack: ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap(),
        });
        let mut owners = std::collections::BTreeMap::from([(uuid, pos)]);
        assert!(items.validate_item_chunk_ownership(pos, &owners).is_ok());
        owners.insert(uuid, other);
        assert!(items.validate_item_chunk_ownership(pos, &owners).is_err());
        owners.insert(uuid, pos);
        items.entities.push(items.entities[0].clone());
        assert!(items.validate_item_chunk_ownership(pos, &owners).is_err());
        items.entities[1]._saved_source = None;
        assert!(items.validate_item_chunk_ownership(pos, &owners).is_err());
        items.entities[1].item.position.x = 0.0;
        assert!(items.validate_item_chunk_ownership(pos, &owners).is_ok());
        assert!(items.validate_item_chunk_ownership(other, &owners).is_err());
    }

    #[test]
    #[ignore = "release-mode local performance measurement"]
    fn measure_spatial_search_against_linear_search() {
        use std::time::Instant;
        for count in [64_usize, 256, 1024] {
            for dense in [false, true] {
                let make = || {
                    let mut items = ItemEntities::default();
                    for index in 0..count {
                        let position = if dense {
                            DVec3::new(
                                (index % 8) as f64 * 0.05,
                                2.0,
                                (index / 8 % 8) as f64 * 0.05,
                            )
                        } else {
                            DVec3::new((index % 32) as f64 * 3.0, 2.0, (index / 32) as f64 * 3.0)
                        };
                        items.entities.push(ItemEntity {
                            item: DroppedItem {
                                entity_id: 0,
                                item_age: 0,
                                position,
                                previous_position: position,
                                stack: ItemStack::new(
                                    format!("test:item_{index}"),
                                    1,
                                    64,
                                    Tag::Compound(vec![]),
                                )
                                .unwrap(),
                                age: 0,
                                bob_offset: 0.0,
                            },
                            velocity: DVec3::ZERO,
                            on_ground: false,
                            ground_without_support: false,
                            pickup_delay: 0,
                            lifetime: 0,
                            no_gravity: true,
                            owner: None,
                            _saved_source: None,
                            created: None,
                            id: index as u32,
                            section_sequence: 0,
                            random: LegacyRandom::new(0),
                            _damage_state: ItemDamageState::default(),
                        });
                    }
                    items
                };
                let mut timings = [Vec::new(), Vec::new()];
                for _ in 0..3 {
                    let mut result = Vec::new();
                    for (i, threshold) in [usize::MAX, 512].into_iter().enumerate() {
                        let mut items = make();
                        let mut inventory = Arc::new(PlayerInventory::default());
                        let start = Instant::now();
                        for _ in 0..80 {
                            items.tick_with_threshold(
                                IVec3::ZERO,
                                DVec3::splat(-1000.0),
                                PlayerPose::Standing,
                                GameMode::Survival,
                                &mut inventory,
                                |_| &[],
                                |_| 0.6,
                                |_| 0.0,
                                |_| false,
                                threshold,
                                |_| native_world::fluid::FluidContacts::default(),
                                false,
                                |_, _, _| None,
                                |_, _| false,
                                |_| Some(false),
                            );
                        }
                        timings[i].push(start.elapsed().as_secs_f64() * 1000.0);
                        result.push(items.snapshot());
                    }
                    assert_eq!(result[0].len(), result[1].len());
                    for (a, b) in result[0].iter().zip(result[1].iter()) {
                        assert_eq!(a.stack, b.stack);
                        assert_eq!(a.position, b.position);
                        assert_eq!(a.age, b.age);
                    }
                }
                for series in &mut timings {
                    series.sort_by(f64::total_cmp);
                }
                eprintln!(
                    "count={count} dense={dense} linear_ms={:.3} indexed_ms={:.3}",
                    timings[0][1], timings[1][1]
                );
            }
        }
    }

    #[test]
    fn spatial_candidates_preserve_linear_merge_order_and_boundaries() {
        let mut original = Vec::new();
        let mut cells = HashMap::<(i64, i64), Vec<usize>>::new();
        for index in 0..200 {
            let position = DVec3::new((index % 20) as f64 - 10.5, 0.0, (index / 20) as f64 - 5.5);
            original.push(ItemEntity {
                item: DroppedItem {
                    entity_id: 0,
                    item_age: 0,
                    position,
                    previous_position: position,
                    stack: ItemStack::new(
                        "minecraft:dirt".into(),
                        (index % 5 + 1) as u32,
                        64,
                        Tag::Compound(vec![]),
                    )
                    .unwrap(),
                    age: 0,
                    bob_offset: 0.0,
                },
                velocity: DVec3::ZERO,
                on_ground: false,
                ground_without_support: false,
                pickup_delay: 0,
                lifetime: 0,
                no_gravity: true,
                owner: None,
                _saved_source: None,
                created: None,
                id: index as u32,
                section_sequence: 0,
                random: LegacyRandom::new(0),
                _damage_state: ItemDamageState::default(),
            });
            cells.entry(item_cell(position)).or_default().push(index);
        }
        // Add a close pair across the negative coordinate cell boundary.
        original[100].item.position = DVec3::new(-0.01, 0.0, -0.01);
        original[101].item.position = DVec3::new(0.01, 0.0, 0.01);
        cells.clear();
        for (index, entity) in original.iter().enumerate() {
            cells
                .entry(item_cell(entity.item.position))
                .or_default()
                .push(index);
        }
        for index in 0..original.len() {
            let candidates =
                item_candidates(&cells, original[index].item.position, usize::MAX).unwrap();
            let mut linear = original.clone();
            let mut indexed = original.clone();
            let mut a = vec![false; original.len()];
            let mut b = a.clone();
            ItemEntities::merge_neighbours(&mut linear, &mut a, &None, index, None);
            ItemEntities::merge_neighbours(&mut indexed, &mut b, &None, index, Some(&candidates));
            assert_eq!(a, b, "entity {index}");
            for (left, right) in linear.iter().zip(&indexed) {
                assert_eq!(left.item.stack, right.item.stack);
                assert_eq!(left.lifetime, right.lifetime);
                assert_eq!(left.pickup_delay, right.pickup_delay);
            }
        }
    }

    #[test]
    fn spatial_index_tracks_an_item_crossing_multiple_cells_before_merging() {
        let mut items = ItemEntities::default();
        for index in 0..512 {
            let position = match index {
                0 => DVec3::new(0.9, 2.0, 0.5),
                1 => DVec3::new(2.2, 2.0, 0.5),
                _ => DVec3::new(100.0 + index as f64 * 3.0, 2.0, 0.5),
            };
            items.entities.push(ItemEntity {
                item: DroppedItem {
                    entity_id: 0,
                    item_age: 0,
                    position,
                    previous_position: position,
                    stack: ItemStack::new(
                        "minecraft:dirt".into(),
                        if index == 0 { 5 } else { 10 },
                        64,
                        Tag::Compound(vec![]),
                    )
                    .unwrap(),
                    age: 1,
                    bob_offset: 0.0,
                },
                velocity: if index == 0 {
                    DVec3::new(1.2, 0.0, 0.0)
                } else {
                    DVec3::ZERO
                },
                on_ground: false,
                ground_without_support: false,
                pickup_delay: 0,
                lifetime: 0,
                no_gravity: true,
                owner: None,
                _saved_source: None,
                created: None,
                id: index as u32,
                section_sequence: 0,
                random: LegacyRandom::new(0),
                _damage_state: ItemDamageState::default(),
            });
        }
        let mut inventory = Arc::new(PlayerInventory::default());
        items.tick(
            IVec3::ZERO,
            DVec3::splat(-1000.0),
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
        );
        assert_eq!(items.entities.len(), 511);
        assert_eq!(items.entities[0].item.stack.count(), 15);
    }

    #[test]
    fn item_merge_stack_limit_and_components_are_respected() {
        let stack = |id: &str, count, max, components| {
            ItemStack::new(id.into(), count, max, components).unwrap()
        };
        let empty = || Tag::Compound(vec![]);
        assert!(stacks_mergeable(
            &stack("minecraft:dirt", 32, 64, empty()),
            &stack("minecraft:dirt", 32, 64, empty())
        ));
        assert!(!stacks_mergeable(
            &stack("minecraft:dirt", 32, 64, empty()),
            &stack("minecraft:dirt", 33, 64, empty())
        ));
        assert!(!stacks_mergeable(
            &stack("minecraft:dirt", 1, 64, empty()),
            &stack("minecraft:stone", 1, 64, empty())
        ));
        assert!(!stacks_mergeable(
            &stack("minecraft:dirt", 1, 64, empty()),
            &stack(
                "minecraft:dirt",
                1,
                64,
                Tag::Compound(vec![(
                    "minecraft:custom_name".into(),
                    Tag::String("name".into())
                )])
            )
        ));
        let unresolved = ItemStack::unresolved("minecraft:dirt".into(), 1, empty()).unwrap();
        assert!(!stacks_mergeable(
            &unresolved,
            &stack("minecraft:dirt", 1, 64, empty())
        ));
    }

    #[test]
    fn stationary_items_merge_on_tick_forty_and_preserve_target_state() {
        let mut items = ItemEntities::default();
        for (count, lifetime, delay) in [(5, 100, 4), (10, 200, 7)] {
            items.entities.push(ItemEntity {
                item: DroppedItem {
                    entity_id: 0,
                    item_age: 0,
                    position: DVec3::ZERO,
                    previous_position: DVec3::ZERO,
                    stack: ItemStack::new(
                        "minecraft:dirt".into(),
                        count,
                        64,
                        Tag::Compound(vec![]),
                    )
                    .unwrap(),
                    age: 0,
                    bob_offset: 0.0,
                },
                velocity: DVec3::ZERO,
                on_ground: false,
                ground_without_support: false,
                pickup_delay: delay,
                lifetime,
                no_gravity: true,
                owner: None,
                _saved_source: None,
                created: None,
                id: count,
                section_sequence: 0,
                random: LegacyRandom::new(0),
                _damage_state: ItemDamageState::default(),
            });
        }
        let mut inventory = Arc::new(PlayerInventory::default());
        for _ in 0..39 {
            items.tick(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Survival,
                &mut inventory,
                |_| &[],
                |_| 0.6,
            );
        }
        assert_eq!(items.entities.len(), 2);
        items.tick(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
        );
        assert_eq!(items.entities.len(), 1);
        assert_eq!(items.entities[0].item.stack.count(), 15);
        assert_eq!(items.entities[0].lifetime, 140);
        assert_eq!(items.entities[0].item.age, 40);
    }

    #[test]
    fn merging_respects_owners_sentinels_and_bounds() {
        let mut items = ItemEntities::default();
        for _ in 0..2 {
            items.entities.push(ItemEntity {
                item: DroppedItem {
                    entity_id: 0,
                    item_age: 0,
                    position: DVec3::ZERO,
                    previous_position: DVec3::ZERO,
                    stack: ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![]))
                        .unwrap(),
                    age: 0,
                    bob_offset: 0.0,
                },
                velocity: DVec3::ZERO,
                on_ground: false,
                ground_without_support: false,
                pickup_delay: 0,
                lifetime: 0,
                no_gravity: true,
                owner: None,
                _saved_source: None,
                created: None,
                id: 0,
                section_sequence: 0,
                random: LegacyRandom::new(0),
                _damage_state: ItemDamageState::default(),
            });
        }
        let mut removed = [false; 2];
        items.entities[1].owner = Some([1, 2, 3, 4]);
        ItemEntities::merge_neighbours(&mut items.entities, &mut removed, &None, 0, None);
        assert_eq!(removed, [false; 2]);
        items.entities[1].owner = None;
        items.entities[1].pickup_delay = i32::from(i16::MAX);
        ItemEntities::merge_neighbours(&mut items.entities, &mut removed, &None, 0, None);
        assert_eq!(removed, [false; 2]);
        items.entities[1].pickup_delay = 0;
        items.entities[1].lifetime = i32::from(i16::MIN);
        ItemEntities::merge_neighbours(&mut items.entities, &mut removed, &None, 0, None);
        assert_eq!(removed, [false; 2]);
        items.entities[1].lifetime = 0;
        items.entities[1].item.position.x = 0.75;
        ItemEntities::merge_neighbours(&mut items.entities, &mut removed, &None, 0, None);
        assert_eq!(removed, [false; 2]);
        items.entities[1].item.position.x = 0.749;
        ItemEntities::merge_neighbours(&mut items.entities, &mut removed, &None, 0, None);
        assert_eq!(removed, [true, false]);
    }

    #[test]
    fn crossing_a_section_appends_to_its_insertion_order() {
        let mut items = ItemEntities::default();
        for (index, (x, count)) in [(15.9, 20), (16.2, 5), (16.3, 10)].into_iter().enumerate() {
            let position = DVec3::new(x, 2.0, 0.5);
            items.entities.push(ItemEntity {
                item: DroppedItem {
                    entity_id: 0,
                    item_age: 0,
                    position,
                    previous_position: position,
                    stack: ItemStack::new(
                        "minecraft:dirt".into(),
                        count,
                        64,
                        Tag::Compound(vec![]),
                    )
                    .unwrap(),
                    age: 0,
                    bob_offset: 0.0,
                },
                velocity: if index == 0 {
                    DVec3::new(0.2, 0.0, 0.0)
                } else {
                    DVec3::ZERO
                },
                on_ground: false,
                ground_without_support: false,
                pickup_delay: 0,
                lifetime: 0,
                no_gravity: true,
                owner: None,
                _saved_source: None,
                created: None,
                id: index as u32,
                section_sequence: index as u64,
                random: LegacyRandom::new(0),
                _damage_state: ItemDamageState::default(),
            });
        }
        items.next_section_sequence = 3;
        let mut inventory = Arc::new(PlayerInventory::default());
        items.tick(
            IVec3::ZERO,
            DVec3::splat(-1000.0),
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
        );
        assert_eq!(items.entities[0].section_sequence, 3);
        let mut removed = [false; 3];
        ItemEntities::merge_neighbours(&mut items.entities, &mut removed, &None, 2, None);
        assert_eq!(removed, [false, true, true]);
        assert_eq!(items.entities[0].item.stack.count(), 35);
    }

    #[test]
    fn chunk_reactivation_resets_tick_count_but_preserves_saved_lifetime() {
        let mut items = ItemEntities::default();
        let home = native_world::loaded::ChunkPos { x: 0, z: 0 };
        let position = DVec3::new(1.0, 2.0, 1.0);
        items.entities.push(ItemEntity {
            item: DroppedItem {
                entity_id: 0,
                item_age: 0,
                position,
                previous_position: position - DVec3::X,
                stack: ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![]))
                    .unwrap(),
                age: 39,
                bob_offset: 0.0,
            },
            velocity: DVec3::ZERO,
            on_ground: false,
            ground_without_support: false,
            pickup_delay: 7,
            lifetime: 123,
            no_gravity: true,
            owner: None,
            _saved_source: None,
            created: None,
            id: 0,
            section_sequence: 4,
            random: LegacyRandom::new(0),
            _damage_state: ItemDamageState::default(),
        });
        items.next_section_sequence = 5;
        items.set_active_chunks(BTreeSet::from([home]));
        items.set_active_chunks(BTreeSet::new());
        items.set_active_chunks(BTreeSet::from([home]));
        let restored = &items.entities[0];
        assert_eq!(restored.item.age, 0);
        assert_eq!(restored.item.previous_position, position);
        assert_eq!(restored.lifetime, 123);
        assert_eq!(restored.pickup_delay, 7);
        assert_eq!(restored.section_sequence, 5);
    }

    #[test]
    fn saved_lifetime_and_pickup_sentinels_are_independent_of_render_age() {
        let mut items = ItemEntities::default();
        let stack = ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        items.entities.push(ItemEntity {
            item: DroppedItem {
                entity_id: 0,
                item_age: 0,
                position: DVec3::ZERO,
                previous_position: DVec3::ZERO,
                stack,
                age: 0,
                bob_offset: 0.0,
            },
            velocity: DVec3::ZERO,
            on_ground: false,
            ground_without_support: false,
            pickup_delay: i32::from(i16::MAX),
            lifetime: i32::from(i16::MIN),
            no_gravity: true,
            owner: None,
            _saved_source: None,
            created: None,
            id: 0,
            section_sequence: 0,
            random: LegacyRandom::new(0),
            _damage_state: ItemDamageState::default(),
        });
        let mut inventory = Arc::new(PlayerInventory::default());
        for _ in 0..6001 {
            items.tick(
                IVec3::ZERO,
                DVec3::ZERO,
                PlayerPose::Standing,
                GameMode::Survival,
                &mut inventory,
                |_| &[],
                |_| 0.6,
            );
        }
        assert_eq!(items.entities.len(), 1);
        let entity = &items.entities[0];
        assert_eq!(entity.lifetime, i32::from(i16::MIN));
        assert_eq!(entity.pickup_delay, i32::from(i16::MAX));
        assert_eq!(entity.item.age, 6001);
        assert_eq!(entity.item.position, DVec3::ZERO);
        items.entities[0].lifetime = -2;
        items.entities[0].pickup_delay = -1;
        items.tick(
            IVec3::ZERO,
            DVec3::ZERO,
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
        );
        assert_eq!(items.entities[0].lifetime, -1);
        assert_eq!(items.entities[0].pickup_delay, -1);
    }

    #[test]
    fn owned_items_require_the_matching_player_even_near_despawn() {
        let mut items = ItemEntities::default();
        let stack = ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        items.entities.push(ItemEntity {
            item: DroppedItem {
                entity_id: 0,
                item_age: 0,
                position: DVec3::ZERO,
                previous_position: DVec3::ZERO,
                stack,
                age: 0,
                bob_offset: 0.0,
            },
            velocity: DVec3::ZERO,
            on_ground: false,
            ground_without_support: false,
            pickup_delay: 0,
            lifetime: 5799,
            no_gravity: true,
            owner: Some([1, 2, 3, 4]),
            _saved_source: None,
            created: None,
            id: 0,
            section_sequence: 0,
            random: LegacyRandom::new(0),
            _damage_state: ItemDamageState::default(),
        });
        let mut inventory = Arc::new(PlayerInventory::default());
        for identity in [None, Some([4, 3, 2, 1])] {
            items.set_player_uuid(identity);
            items.tick(
                IVec3::ZERO,
                DVec3::ZERO,
                PlayerPose::Standing,
                GameMode::Survival,
                &mut inventory,
                |_| &[],
                |_| 0.6,
            );
            assert_eq!(items.entities.len(), 1);
        }
        items.set_player_uuid(Some([1, 2, 3, 4]));
        items.tick(
            IVec3::ZERO,
            DVec3::ZERO,
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
        );
        assert!(items.entities.is_empty());
    }

    struct LootSource;
    impl ResourceProvider for LootSource {
        fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
            let item = if path.ends_with("/dirt.json") || path.ends_with("/grass_block.json") {
                "dirt"
            } else if path.ends_with("/stone.json") || path.ends_with("/cobblestone.json") {
                "cobblestone"
            } else if path.ends_with("/stone_bricks.json") {
                "stone_bricks"
            } else {
                return Ok(None);
            };
            Ok(Some(format!(r#"{{"type":"minecraft:block","pools":[{{"rolls":1,"entries":[{{"type":"minecraft:item","name":"minecraft:{item}"}}]}}]}}"#).into_bytes()))
        }
    }

    fn tables() -> BlockLootTables {
        BlockLootTables::load(&mut LootSource).unwrap()
    }

    #[test]
    fn loot_obeys_game_mode_tool_and_world_rule() {
        let loot = tables();
        let mut items = ItemEntities::default();
        for (mode, enabled, name) in [
            (GameMode::Creative, true, "minecraft:dirt"),
            (GameMode::Survival, false, "minecraft:dirt"),
            (GameMode::Survival, true, "minecraft:stone"),
        ] {
            items.drop_block(IVec3::ZERO, name, None, mode, enabled, Some(&loot));
        }
        assert!(items.snapshot().is_empty());
        items.drop_block(
            IVec3::ZERO,
            "minecraft:grass_block",
            None,
            GameMode::Survival,
            true,
            Some(&loot),
        );
        assert_eq!(items.snapshot()[0].stack.id(), "minecraft:dirt");
    }

    #[test]
    fn pickaxe_drops_resolved_cobblestone_and_pickup_can_store_it() {
        let loot = tables();
        let mut items = ItemEntities::default();
        let pickaxe = ItemStack::new(
            "minecraft:copper_pickaxe".into(),
            1,
            1,
            Tag::Compound(vec![]),
        )
        .unwrap();
        items.drop_block(
            IVec3::ZERO,
            "minecraft:stone",
            Some(&pickaxe),
            GameMode::Survival,
            true,
            Some(&loot),
        );
        let drop = &items.snapshot()[0].stack;
        assert_eq!(drop.id(), "minecraft:cobblestone");
        assert_eq!(drop.max_count(), Some(64));
        let mut inventory = PlayerInventory::default();
        let mut incoming = Some(drop.clone());
        assert!(inventory.pickup_stack(&mut incoming, false));
        assert!(incoming.is_none());
        assert_eq!(
            inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap()
                .id(),
            "minecraft:cobblestone"
        );
    }

    #[test]
    fn pickup_waits_for_delay_and_old_snapshots_are_unchanged() {
        let loot = tables();
        let mut items = ItemEntities::default();
        let mut inventory = Arc::new(PlayerInventory::default());
        items.drop_block(
            IVec3::ZERO,
            "minecraft:dirt",
            None,
            GameMode::Survival,
            true,
            Some(&loot),
        );
        let before = items.snapshot();
        for tick in 0..10 {
            items.tick(
                IVec3::ZERO,
                DVec3::new(0.5, 0.0, 0.5),
                PlayerPose::Standing,
                GameMode::Survival,
                &mut inventory,
                |pos| {
                    if pos.y < 0 {
                        native_world::collision::unknown_barrier()
                    } else {
                        &[]
                    }
                },
                |_| 0.6,
            );
            if tick < 9 {
                assert_eq!(items.snapshot().len(), 1);
            }
        }
        assert!(items.snapshot().is_empty());
        assert_eq!(
            inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap()
                .id(),
            "minecraft:dirt"
        );
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].age, 0);
    }

    #[test]
    fn dropped_visual_metadata_tracks_item_lifetime_and_session_identity() {
        let mut items = ItemEntities::default();
        let mut first = test_saved_item([1, 2, 3, 4], 1);
        first.position = DVec3::new(1.5, 5.0, 1.5);
        first.age = 37;
        let mut second = test_saved_item([5, 6, 7, 8], 1);
        second.position = DVec3::new(10.5, 5.0, 10.5);
        second.age = i16::MIN;
        items.spawn_saved(first);
        items.spawn_saved(second);
        let before = items.snapshot();
        assert_ne!(before[0].entity_id, before[1].entity_id);
        assert_eq!((before[0].age, before[0].item_age), (0, 37));
        assert_eq!(
            (before[1].age, before[1].item_age),
            (0, i32::from(i16::MIN))
        );
        let mut inventory = Arc::new(PlayerInventory::default());
        items.tick(
            IVec3::ZERO,
            DVec3::new(100.0, 0.0, 100.0),
            PlayerPose::Standing,
            GameMode::Spectator,
            &mut inventory,
            |_| &[],
            |_| 0.6,
        );
        let after = items.snapshot();
        assert_eq!((after[0].age, after[0].item_age), (1, 38));
        assert_eq!((after[1].age, after[1].item_age), (1, i32::from(i16::MIN)));
        assert_eq!(before[0].item_age, 37);
        assert_eq!(before[0].entity_id, after[0].entity_id);
    }

    #[test]
    fn player_thrown_items_wait_forty_ticks_before_pickup() {
        let mut items = ItemEntities::default();
        let mut inventory = Arc::new(PlayerInventory::default());
        let player = PlayerState {
            position: glam::Vec3::new(0.5, 0.0, 0.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: true,
            pose: PlayerPose::Standing,
        };
        let stack = ItemStack::new("minecraft:dirt".into(), 3, 64, Tag::Compound(vec![])).unwrap();
        items.drop_player(
            stack,
            IVec3::ZERO,
            player.position.as_dvec3(),
            player,
            PlayerDropOrigin::ContainerReturn,
        );
        items.entities[0].item.position = DVec3::new(0.5, 0.0, 0.5);
        items.entities[0].velocity = DVec3::ZERO;
        for tick in 0..40 {
            items.tick(
                IVec3::ZERO,
                player.position.as_dvec3(),
                player.pose,
                GameMode::Survival,
                &mut inventory,
                |pos| {
                    if pos.y < 0 {
                        native_world::collision::unknown_barrier()
                    } else {
                        &[]
                    }
                },
                |_| 0.6,
            );
            if tick < 39 {
                assert_eq!(items.snapshot().len(), 1, "tick {tick}");
            }
        }
        assert!(items.snapshot().is_empty());
        assert_eq!(
            inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            3
        );
    }

    #[test]
    fn dropped_item_snapshot_keeps_previous_tick_position_and_stable_phase() {
        let mut items = ItemEntities::default();
        let player = PlayerState {
            position: glam::Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: true,
            pose: PlayerPose::Standing,
        };
        let stack = ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        items.drop_player(
            stack.clone(),
            IVec3::ZERO,
            DVec3::ZERO,
            player,
            PlayerDropOrigin::ContainerReturn,
        );
        items.drop_player(
            stack,
            IVec3::ZERO,
            DVec3::ZERO,
            player,
            PlayerDropOrigin::ContainerReturn,
        );
        let initial = items.snapshot();
        assert_eq!(initial[0].previous_position, initial[0].position);
        assert_ne!(initial[0].bob_offset, initial[1].bob_offset);
        let mut inventory = Arc::new(PlayerInventory::default());
        items.tick(
            IVec3::ZERO,
            DVec3::splat(1000.0),
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
        );
        let next = items.snapshot();
        for (before, after) in initial.iter().zip(next.iter()) {
            assert_eq!(after.previous_position, before.position);
            assert_ne!(after.position, before.position);
            assert_eq!(after.bob_offset, before.bob_offset);
        }
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn chest_content_scattering_matches_java_random_order() {
        use serde_json::Value;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let loot = BlockLootTables::load(&mut resources).unwrap();
        let tool = BasicTool {
            item: None,
            count: 0,
            enchantments: &[],
        };
        for name in ["minecraft:chest", "minecraft:trapped_chest"] {
            let drops = loot.drops(name, tool).unwrap();
            assert_eq!(drops.len(), 1);
            assert_eq!(
                format!("{}:{}", drops[0].item.namespace, drops[0].item.path),
                name
            );
            assert!(drops[0].copy_block_entity_custom_name);
        }
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("ChestDropOracle.java");
        let output = temp.path().join("drops.json");
        std::fs::write(&source, include_str!("../tests/java/ChestDropOracle.java")).unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
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
        let cp = std::env::join_paths(paths).unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("ChestDropOracle")
            .arg(&output));
        for (trapped, id) in [
            (false, "minecraft:chest"),
            (true, "minecraft:trapped_chest"),
        ] {
            for styled in [false, true] {
                let path = output.with_file_name(format!("drops.json.{trapped}.{styled}.nbt"));
                let java_tag = native_persistence::nbt::read(&std::fs::read(path).unwrap())
                    .unwrap()
                    .value;
                let item_path =
                    output.with_file_name(format!("drops.json.{trapped}.{styled}.item.nbt"));
                let item_tag = native_persistence::nbt::read(&std::fs::read(item_path).unwrap())
                    .unwrap()
                    .value;
                let stack = crate::player_data::read_stack(&item_tag).unwrap();
                assert_eq!(stack.max_count(), Some(64));
                let name = stack
                    .components()
                    .get("minecraft:custom_name")
                    .unwrap()
                    .clone();
                assert_eq!(java_tag.get("CustomName"), Some(&name));
                if !styled {
                    assert_eq!(name, Tag::String("보관함 ♠".into()));
                }
                let container = stack.components().get("minecraft:container").unwrap();
                let chest = crate::saved_chest::SavedChest::placed(
                    IVec3::ZERO,
                    id,
                    Some(name.clone()),
                    Some(container),
                )
                .unwrap();
                assert_eq!(chest.custom_name(), Some(&name));
                assert_eq!(chest.source().get("id"), java_tag.get("id"));
                let document = native_persistence::nbt::NamedTag {
                    name: "".into(),
                    value: Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![java_tag],
                    },
                };
                let payload = native_world::loaded::SavedBlockEntities::Encoded(
                    native_persistence::nbt::write(&document).unwrap().into(),
                );
                let java_chest = crate::saved_chest::read(
                    &payload,
                    native_world::loaded::ChunkPos { x: 0, z: 0 },
                    IVec3::ZERO,
                )
                .unwrap()
                .unwrap();
                assert_eq!(chest.slots, java_chest.slots);
            }
        }
        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        assert_eq!(rows.len(), 32);
        for (seed, row) in rows.iter().enumerate() {
            let mut items = ItemEntities::default();
            let mut random = LegacyRandom::new(seed as i64);
            let counts = [1, 9, 10, 11, 29, 30, 31, 64];
            let slots: Vec<_> = (0..27)
                .map(|i| {
                    ((i + seed) % 4 == 0).then(|| {
                        ItemStack::new(
                            "minecraft:dirt".into(),
                            counts[(i / 4 + seed) % 8],
                            64,
                            Tag::Compound(vec![]),
                        )
                        .unwrap()
                    })
                })
                .collect();
            let pos = IVec3::new(
                if seed % 2 == 0 {
                    -16_000_000
                } else {
                    16_000_000
                },
                seed as i32 - 64,
                -(seed as i32) * 31,
            );
            items.drop_container_contents_with_random(pos, &slots, &mut random);
            let expected = row["drops"].as_array().unwrap();
            assert_eq!(items.entities.len(), expected.len(), "seed {seed}");
            for (item, row) in items.entities.iter().zip(expected) {
                assert_eq!(
                    u64::from(item.item.stack.count()),
                    row["count"].as_u64().unwrap()
                );
                assert_eq!(item.pickup_delay, row["delay"].as_i64().unwrap() as i32);
                for (actual, field) in [
                    (item.item.position.x, "x"),
                    (item.item.position.y, "y"),
                    (item.item.position.z, "z"),
                    (item.velocity.x, "vx"),
                    (item.velocity.y, "vy"),
                    (item.velocity.z, "vz"),
                ] {
                    assert_eq!(
                        actual.to_bits(),
                        row[field].as_f64().unwrap().to_bits(),
                        "seed {seed} {field}"
                    );
                }
            }
            assert_eq!(
                random.next_double().to_bits(),
                row["next"].as_f64().unwrap().to_bits(),
                "seed {seed} random"
            );
            for enabled in [false, true] {
                let mut block_random = random.clone();
                let mut block_items = ItemEntities::default();
                block_items.drop_block_resources(
                    pos,
                    vec![Drop {
                        item: native_resources::packs::ResourceId::parse("minecraft:chest")
                            .unwrap(),
                        count: 1,
                        copy_block_entity_custom_name: false,
                        copy_block_entity_components: vec![],
                    }],
                    Some(&[Tag::Compound(vec![])]),
                    enabled,
                    &mut block_random,
                );
                assert_eq!(block_items.entities.len(), usize::from(enabled));
                assert_eq!(
                    block_random.next_double().to_bits(),
                    row["blockNext"].as_f64().unwrap().to_bits(),
                    "seed {seed} block random enabled={enabled}"
                );
            }
        }
        let faces: Vec<Value> = serde_json::from_slice(
            &std::fs::read(output.with_file_name("drops.json.faces.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(faces.len(), 16 * 6 * 2);
        for row in faces {
            let seed = row["seed"].as_i64().unwrap();
            let face = match row["face"].as_str().unwrap() {
                "down" => IVec3::NEG_Y,
                "up" => IVec3::Y,
                "north" => IVec3::NEG_Z,
                "south" => IVec3::Z,
                "west" => IVec3::NEG_X,
                "east" => IVec3::X,
                _ => unreachable!(),
            };
            let enabled = row["enabled"].as_bool().unwrap();
            let pos = IVec3::new(
                if seed % 2 == 0 {
                    -16_000_000
                } else {
                    16_000_000
                },
                seed as i32 - 64,
                -(seed as i32) * 31,
            );
            let mut random = LegacyRandom::new(seed);
            let mut items = ItemEntities::default();
            items.drop_interaction_with_level_random(
                pos,
                face,
                DropStrategy::ClickedFace,
                vec![Drop {
                    item: native_resources::packs::ResourceId::parse("minecraft:dirt").unwrap(),
                    count: 1,
                    copy_block_entity_custom_name: false,
                    copy_block_entity_components: vec![],
                }],
                enabled,
                &mut random,
            );
            assert_eq!(items.entities.len(), usize::from(enabled));
            assert_eq!(
                random.next_double().to_bits(),
                row["next"].as_f64().unwrap().to_bits(),
                "seed={seed} face={face:?} enabled={enabled}"
            );
            if let Some(item) = items.entities.first() {
                for (actual, field) in [
                    (item.item.position.x, "x"),
                    (item.item.position.y, "y"),
                    (item.item.position.z, "z"),
                    (item.velocity.x, "vx"),
                    (item.velocity.y, "vy"),
                    (item.velocity.z, "vz"),
                ] {
                    assert_eq!(
                        actual.to_bits(),
                        row[field].as_f64().unwrap().to_bits(),
                        "seed={seed} face={face:?} field={field}"
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn initial_player_drop_matches_java_with_the_same_random_source() {
        use serde_json::{Value, json};
        use std::{
            ffi::OsString,
            path::{Path, PathBuf},
            process::Command,
        };
        const JAVA: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;

public class PlayerDropOracle {
  static class TestLevel extends ClientLevel { TestLevel() { super(null,null,null,null,0,0,null,false,0L,0); } }
  static class TestPlayer extends Player {
    Level world; float yaw, pitch; double eye;
    TestPlayer() { super(null,null); }
    @Override public Level level() { return world; }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public float getYRot() { return yaw; }
    @Override public float getXRot() { return pitch; }
    @Override public double getEyeY() { return eye; }
  }
  static JsonArray vec(Vec3 v) { var a = new JsonArray(); a.add(v.x); a.add(v.y); a.add(v.z); return a; }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    Items.DIRT.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
    var u = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); u.setAccessible(true); var unsafe = (sun.misc.Unsafe) u.get(null);
    var position = Entity.class.getDeclaredField("position"); position.setAccessible(true);
    var random = Entity.class.getDeclaredField("random"); random.setAccessible(true);
    var delay = ItemEntity.class.getDeclaredField("pickupDelay"); delay.setAccessible(true);
    var thrower = ItemEntity.class.getDeclaredField("thrower"); thrower.setAccessible(true);
    var inputs = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var output = new JsonArray();
    for (var value : inputs) {
      var c = value.getAsJsonObject();
      var world = (TestLevel) unsafe.allocateInstance(TestLevel.class);
      var player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
      player.world = world; player.yaw = c.get("yaw").getAsFloat(); player.pitch = c.get("pitch").getAsFloat();
      var p = c.getAsJsonArray("position");
      position.set(player,new Vec3(p.get(0).getAsDouble(),p.get(1).getAsDouble(),p.get(2).getAsDouble()));
      player.eye = p.get(1).getAsDouble() + c.get("eyeHeight").getAsDouble();
      random.set(player,new LegacyRandomSource(c.get("seed").getAsLong()));
      player.setUUID(new java.util.UUID(0x0000000100000002L,0x0000000300000004L));
      var item = player.createItemStackToDrop(new ItemStack(Items.DIRT),false,c.get("trace").getAsBoolean());
      if (item.fallDistance != 0 || item.getAirSupply() != 300 || item.getPortalCooldown() != 0 ||
          item.getYRot() < 0 || item.getYRot() >= 360 || item.getXRot() != 0)
        throw new AssertionError("unexpected initial item base fields");
      var result = new JsonObject(); result.add("position",vec(new Vec3(item.getX(),item.getY(),item.getZ())));
      var ref = (net.minecraft.world.entity.EntityReference<?>)thrower.get(item);
      result.addProperty("thrower",ref == null ? "" : ref.getUUID().toString());
      result.add("velocity",vec(item.getDeltaMovement())); result.addProperty("delay",delay.getInt(item)); output.add(result);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(output));
  }
}
"#;
        fn classpath(game: &Path, classes: &Path) -> OsString {
            let version = game.join("versions/26.3");
            let manifest: Value =
                serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
            let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
            for library in manifest["libraries"].as_array().unwrap() {
                if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                    let path = game.join("libraries").join(path);
                    if path.is_file() {
                        paths.push(path);
                    }
                }
            }
            std::env::join_paths(paths).unwrap()
        }
        fn run(command: &mut Command) {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "drop oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        fn vec(value: &Value) -> DVec3 {
            DVec3::new(
                value[0].as_f64().unwrap(),
                value[1].as_f64().unwrap(),
                value[2].as_f64().unwrap(),
            )
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("PlayerDropOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        let mut players = Vec::new();
        let mut cases = Vec::new();
        for index in 0..96 {
            let pose = if index % 2 == 0 {
                PlayerPose::Standing
            } else {
                PlayerPose::Crouching
            };
            let player = PlayerState {
                position: glam::Vec3::new(3.5, 64.0, -8.25),
                yaw: (index as f32 * 23.0 - 180.0).to_radians(),
                pitch: ((index % 7) as f32 * 22.0 - 66.0).to_radians(),
                vertical_velocity: 0.0,
                on_ground: true,
                pose,
            };
            cases.push(json!({"seed":index as i64 + 31, "yaw":player.yaw.to_degrees()-180.0, "pitch":-player.pitch.to_degrees(), "eyeHeight":f64::from(pose.eye_height()), "position":player.position.to_array(), "trace":index % 2 == 0}));
            players.push(player);
        }
        std::fs::write(&source, JAVA).unwrap();
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        let cp = classpath(&game, temp.path());
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("PlayerDropOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        for (index, (player, expected)) in players.into_iter().zip(expected.iter()).enumerate() {
            let mut items = ItemEntities {
                random: LegacyRandom::new(index as i64 + 31),
                ..Default::default()
            };
            let stack =
                ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![])).unwrap();
            items.set_player_uuid(Some([1, 2, 3, 4]));
            let traced = index % 2 == 0;
            items.drop_player(
                stack,
                IVec3::ZERO,
                player.position.as_dvec3(),
                player,
                if traced {
                    PlayerDropOrigin::Direct
                } else {
                    PlayerDropOrigin::ContainerReturn
                },
            );
            let entity = &items.entities[0];
            let expected_thrower = if traced {
                "00000001-0000-0002-0000-000300000004"
            } else {
                ""
            };
            assert_eq!(expected["thrower"].as_str().unwrap(), expected_thrower);
            assert_eq!(
                entity.created.as_ref().unwrap().thrower,
                CreatedThrower::Known(traced.then_some([1, 2, 3, 4]))
            );
            let p_error = (entity.item.position - vec(&expected["position"]))
                .abs()
                .max_element();
            let v_error = (entity.velocity - vec(&expected["velocity"]))
                .abs()
                .max_element();
            assert!(
                p_error < 1.0e-10
                    && v_error < 1.0e-7
                    && entity.pickup_delay == expected["delay"].as_i64().unwrap() as i32,
                "case {index}: p {:?} v {:?}, expected {expected}",
                entity.item.position,
                entity.velocity
            );
        }
    }

    #[test]
    fn item_collision_and_despawn_are_tick_driven() {
        let loot = tables();
        let mut items = ItemEntities::default();
        let mut inventory = Arc::new(PlayerInventory::default());
        items.drop_block(
            IVec3::ZERO,
            "minecraft:dirt",
            None,
            GameMode::Survival,
            true,
            Some(&loot),
        );
        for _ in 0..100 {
            items.tick(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Survival,
                &mut inventory,
                |pos| {
                    if pos.y < 0 {
                        native_world::collision::unknown_barrier()
                    } else {
                        &[]
                    }
                },
                |_| 0.6,
            );
        }
        let item = &items.snapshot()[0];
        assert!(
            item.position.y >= -1.0e-6 && item.position.y < 0.01,
            "{item:?}"
        );
        items.entities[0].lifetime = 5999;
        items.tick(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Survival,
            &mut inventory,
            |_| &[],
            |_| 0.6,
        );
        assert!(items.snapshot().is_empty());
    }

    #[test]
    fn mining_dirt_spawns_then_collects_an_item_in_a_live_session() {
        use crate::{GameSession, LocalSession, PlayerCommand, TICK};
        use native_world::{Block, World};
        let mut world = World::default();
        for x in -3..=3 {
            for z in 0..=7 {
                world.set_block(IVec3::new(x, 2, z), Block::Stone);
            }
        }
        let target = IVec3::new(0, 4, 3);
        world.set_block(target, Block::Dirt);
        let mut session = LocalSession::new(world);
        session.set_block_loot(Arc::new(tables()));
        session.set_player_inventory(PlayerInventory::default());
        session.update(TICK * 3);
        let original = session.snapshot();
        session.submit_command(PlayerCommand::Break);
        let mut spawned = None;
        for _ in 0..30 {
            session.update(TICK);
            let snapshot = session.snapshot();
            if !snapshot.dropped_items.is_empty() {
                spawned = Some(snapshot);
                break;
            }
        }
        let spawned = spawned.expect("dirt should spawn a drop");
        assert_eq!(original.world.demo().unwrap().block(target), Block::Dirt);
        assert_eq!(spawned.world.demo().unwrap().block(target), Block::Air);
        assert_eq!(spawned.dropped_items[0].stack.id(), "minecraft:dirt");
        session.submit_command(PlayerCommand::StopBreak);
        session.motion.position = DVec3::new(0.5, 3.0, 4.0);
        for _ in 0..20 {
            session.update(TICK);
        }
        let after = session.snapshot();
        assert!(after.dropped_items.is_empty());
        assert_eq!(
            after
                .inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap()
                .id(),
            "minecraft:dirt"
        );
        assert_eq!(spawned.dropped_items.len(), 1);
    }

    #[test]
    fn mine_pick_up_and_place_cobblestone_in_one_session() {
        use crate::{GameSession, LocalSession, PlayerCommand, TICK};
        use native_world::{Block, World};
        let mut world = World::default();
        for x in -3..=3 {
            for z in 0..=7 {
                world.set_block(IVec3::new(x, 2, z), Block::Stone);
            }
        }
        let target = IVec3::new(0, 4, 3);
        world.set_block(target, Block::Stone);
        world.set_block(IVec3::new(0, 4, 2), Block::Stone);
        let mut session = LocalSession::new(world);
        session.set_block_loot(Arc::new(tables()));
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            crate::inventory::PlayerSlot::Storage(0),
            ItemStack::new(
                "minecraft:wooden_pickaxe".into(),
                1,
                1,
                Tag::Compound(vec![]),
            ),
        );
        session.set_player_inventory(inventory);
        session.update(TICK * 3);
        session.submit_command(PlayerCommand::Break);
        for _ in 0..30 {
            session.update(TICK);
            if !session.snapshot().dropped_items.is_empty() {
                break;
            }
        }
        assert_eq!(
            session.snapshot().dropped_items[0].stack.id(),
            "minecraft:cobblestone"
        );
        session.submit_command(PlayerCommand::StopBreak);
        session.motion.position = DVec3::new(0.5, 3.0, 4.0);
        for _ in 0..20 {
            session.update(TICK);
        }
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(crate::inventory::PlayerSlot::Storage(1))
                .unwrap()
                .id(),
            "minecraft:cobblestone"
        );
        session.motion.position = DVec3::new(0.5, 3.0, 5.5);
        session.update(TICK);
        session.submit_command(PlayerCommand::SelectSlot(1));
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        let after = session.snapshot();
        assert_eq!(
            after.world.demo().unwrap().block(target),
            Block::Cobblestone
        );
        assert!(
            after
                .inventory
                .slot(crate::inventory::PlayerSlot::Storage(1))
                .is_none()
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn mine_log_pick_it_up_craft_and_place_planks() {
        use crate::{
            GameSession, LocalSession, PlayerCommand, TICK,
            inventory::{ClickButton, PlayerSlot},
        };
        use native_resources::{packs::ResourceStack, recipes::CraftingRecipes};
        use native_world::{Axis, Block, World};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let loot = Arc::new(BlockLootTables::load(&mut resources).unwrap());
        let recipes = Arc::new(CraftingRecipes::load(&mut resources).unwrap());
        let wood = native_core::wood::Wood::from_id("minecraft:oak_log").unwrap();
        let mut world = World::default();
        for x in -3..=3 {
            for z in 0..=7 {
                world.set_block(IVec3::new(x, 2, z), Block::Stone);
            }
        }
        let target = IVec3::new(0, 4, 3);
        world.set_block(target, Block::Wood(wood, Axis::Y));
        world.set_block(IVec3::new(0, 4, 2), Block::Stone);
        let mut session = LocalSession::new(world);
        session.set_block_loot(loot);
        session.set_crafting_recipes(recipes);
        session.set_player_inventory(PlayerInventory::default());
        session.update(TICK * 3);
        session.submit_command(PlayerCommand::Break);
        for _ in 0..100 {
            session.update(TICK);
            if !session.snapshot().dropped_items.is_empty() {
                break;
            }
        }
        assert_eq!(
            session.snapshot().dropped_items[0].stack.id(),
            "minecraft:oak_log"
        );
        session.submit_command(PlayerCommand::StopBreak);
        session.motion.position = DVec3::new(0.5, 3.0, 4.0);
        for _ in 0..20 {
            session.update(TICK);
        }
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .id(),
            "minecraft:oak_log"
        );
        session.submit_command(PlayerCommand::ToggleInventory);
        session.update(TICK);
        for slot in [
            PlayerSlot::Storage(0),
            PlayerSlot::Crafting(0),
            PlayerSlot::CraftResult,
            PlayerSlot::Storage(0),
        ] {
            session.submit_command(PlayerCommand::InventoryClick {
                slot,
                button: ClickButton::Primary,
            });
            session.update(TICK);
        }
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            4
        );
        session.submit_command(PlayerCommand::ToggleInventory);
        session.motion.position = DVec3::new(0.5, 3.0, 5.5);
        session.update(TICK);
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        let after = session.snapshot();
        assert_eq!(
            after.world.demo().unwrap().block(target),
            Block::Planks(native_core::wood::Planks::Oak)
        );
        assert_eq!(
            after
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            3
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn active_world_data_pack_changes_the_spawned_loot() {
        use native_resources::{datapacks::DataPackSettings, packs::ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let world = temp.path().join("world");
        let pack = world.join("datapacks/custom");
        std::fs::create_dir_all(pack.join("data/minecraft/loot_table/blocks")).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        std::fs::write(pack.join("data/minecraft/loot_table/blocks/dirt.json"), br#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone_bricks","modifier":{"type":"minecraft:set_count","count":2}}]}]}"#).unwrap();
        std::fs::write(pack.join("data/minecraft/loot_table/blocks/oak_planks.json"), br#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:crafting_table"}]}]}"#).unwrap();
        let mut source = ResourceStack::open(&game).unwrap();
        source
            .configure_world_data(
                &world,
                &DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/custom".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )
            .unwrap();
        let loot = BlockLootTables::load(&mut source).unwrap();
        let mut items = ItemEntities::default();
        items.drop_block(
            IVec3::ZERO,
            "minecraft:dirt",
            None,
            GameMode::Survival,
            true,
            Some(&loot),
        );
        let drops = items.snapshot();
        assert_eq!(
            (drops[0].stack.id(), drops[0].stack.count()),
            ("minecraft:stone_bricks", 2)
        );
        items.drop_block(
            IVec3::X,
            "minecraft:oak_planks",
            None,
            GameMode::Survival,
            true,
            Some(&loot),
        );
        let drops = items.snapshot();
        assert_eq!(
            (
                drops[1].stack.id(),
                drops[1].stack.count(),
                drops[1].stack.max_count()
            ),
            ("minecraft:crafting_table", 1, Some(64))
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn airborne_and_flat_ground_item_motion_match_java() {
        use serde_json::{Value, json};
        use std::{
            ffi::OsString,
            path::{Path, PathBuf},
            process::Command,
        };
        const JAVA: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.MoverType;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.AABB;
import net.minecraft.core.SectionPos;
import net.minecraft.world.level.entity.EntitySectionStorage;
import net.minecraft.world.level.entity.Visibility;
import net.minecraft.util.Continuation;
import net.minecraft.tags.TagKey;
import net.minecraft.world.level.material.Fluid;

public class ItemMotionOracle {
    static final Field POSITION;
    static final Method RESTITUTE;
    static { try { POSITION = Entity.class.getDeclaredField("position"); POSITION.setAccessible(true);
                   RESTITUTE=Entity.class.getDeclaredMethod("restituteMovementAfterCollisions", BlockState.class, boolean.class, boolean.class, Vec3.class); RESTITUTE.setAccessible(true); }
             catch (Exception e) { throw new RuntimeException(e); } }
    static class TestLevel extends ClientLevel {
        BlockState ground; boolean supportGrid; boolean twoBlocks; boolean server;
        TestLevel() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public boolean isClientSide() { return !server; }
        @Override public boolean noCollision(Entity entity, AABB box) { return true; }
        @Override public void playSound(Entity source, BlockPos pos, net.minecraft.sounds.SoundEvent sound, net.minecraft.sounds.SoundSource category, float volume, float pitch) { }
        @Override public BlockState getBlockState(BlockPos pos) {
            if (!supportGrid) return ground;
            return pos.getY()==0 && pos.getZ()==0 && (pos.getX()==0 || twoBlocks && pos.getX()==1)
                ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
        }
        @Override public net.minecraft.world.level.BlockGetter getChunkForCollisions(int x,int z) { return this; }
    }
    static class TestItem extends ItemEntity {
        Vec3 velocity; boolean grounded; boolean noGravity; Level world; int identity;
        ItemStack stack; boolean removed; int fluidKind;
        TestItem() { super(null,0,0,0,ItemStack.EMPTY); }
        void pos(Vec3 p) { try { POSITION.set(this,p); } catch (Exception e) { throw new RuntimeException(e); } }
        @Override public Level level() { return world; }
        @Override public ItemStack getItem() { return stack == null ? new ItemStack(Items.DIRT) : stack; }
        @Override public void setItem(ItemStack value) { stack = value; }
        @Override public void remove(Entity.RemovalReason reason) { removed = true; }
        @Override public void baseTick() {
            // Preserve the fall-distance effects from Entity.baseTick while
            // avoiding unrelated client world and fire machinery in this mock.
            if (isInWater()) resetFallDistance();
            if (isInLava()) fallDistance *= 0.5;
        }
        @Override public boolean isInWater() { return fluidKind == 1; }
        @Override public boolean isInLava() { return fluidKind == 2; }
        @Override public double getFluidHeight(TagKey<Fluid> tag) { return 1.0; }
        @Override public boolean isNoGravity() { return noGravity; }
        @Override public boolean isSuppressingBounce() { return false; }
        @Override public boolean isDescending() { return false; }
        @Override public void gameEvent(net.minecraft.core.Holder<net.minecraft.world.level.gameevent.GameEvent> event) { }
        @Override public boolean onGround() { return grounded; }
        @Override public int getId() { return identity; }
        @Override public Vec3 getDeltaMovement() { return velocity; }
        @Override public void setDeltaMovement(Vec3 v) { velocity = v; }
        @Override public BlockPos getBlockPosBelowThatAffectsMyMovement() { return BlockPos.ZERO; }
        @Override protected void applyEffectsFromBlocks() { }
        @Override protected void applyEffectsFromBlocksForLastMovements() { }
        @Override protected boolean updateFluidInteraction() { return false; }
        @Override public void move(MoverType type, Vec3 requested) {
            double y = requested.y;
            if (y < 0 && getY() + y < 0) y = -getY();
            boolean blocked = y != requested.y;
            pos(new Vec3(getX() + requested.x, getY() + y, getZ() + requested.z));
            grounded = blocked && requested.y < 0;
            // The real Entity.move calls this with collision-adjusted Y.
            // Landing also triggers block effects, which this mock omits.
            if (grounded) resetFallDistance();
            else checkFallDamage(y, false, world.getBlockState(BlockPos.ZERO), BlockPos.ZERO);
            if (blocked) {
                verticalCollision=true; verticalCollisionBelow=grounded;
                try { RESTITUTE.invoke(this,world.getBlockState(BlockPos.ZERO),false,false,new Vec3(requested.x,y,requested.z)); }
                catch (Exception e) { throw new RuntimeException(e); }
            }
        }
    }
    static class TestPlayer extends net.minecraft.world.entity.player.Player {
        Vec3 velocity; boolean suppress; double bounciness;
        TestPlayer() { super(null,null); }
        void setWorld(Level world) { setLevel(world); }
        @Override public Vec3 getDeltaMovement() { return velocity; }
        @Override public void setDeltaMovement(Vec3 v) { velocity=v; }
        @Override protected double getEffectiveGravity() { return 0.08; }
        @Override protected double getEntityBounciness() { return bounciness; }
        @Override protected float getAirDrag() { return 0.98f; }
        @Override public boolean isSuppressingBounce() { return suppress; }
        @Override public void gameEvent(net.minecraft.core.Holder<net.minecraft.world.level.gameevent.GameEvent> event) { }
        @Override public BlockPos getBlockPosBelowThatAffectsMyMovement() { return BlockPos.ZERO; }
        @Override public net.minecraft.world.entity.HumanoidArm getMainArm() { return net.minecraft.world.entity.HumanoidArm.RIGHT; }
        @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
    }
    static JsonArray vector(Vec3 v) { JsonArray a = new JsonArray(); a.add(v.x); a.add(v.y); a.add(v.z); return a; }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Items.DIRT.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
        var f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) f.get(null);
        var age = ItemEntity.class.getDeclaredField("age"); age.setAccessible(true);
        var delay = ItemEntity.class.getDeclaredField("pickupDelay"); delay.setAccessible(true);
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray output = new JsonArray();
        for (var value : cases) {
            var input = value.getAsJsonObject();
            var world = (TestLevel) unsafe.allocateInstance(TestLevel.class);
            world.server = input.has("server") && input.get("server").getAsBoolean();
            int block = input.get("block").getAsInt();
            world.ground = (block==1 ? Blocks.SLIME_BLOCK : block==2 ? net.minecraft.core.registries.BuiltInRegistries.BLOCK.getValue(net.minecraft.resources.Identifier.parse("minecraft:red_bed")) :
                block==3 ? Blocks.SHELF_MUSHROOM : input.get("ice").getAsBoolean() ? Blocks.ICE : Blocks.STONE).defaultBlockState();
            var item = (TestItem) unsafe.allocateInstance(TestItem.class);
            item.world = world; item.identity = input.get("id").getAsInt();
            item.noGravity = input.get("no_gravity").getAsBoolean();
            item.fluidKind = input.get("fluid").getAsInt();
            age.setInt(item,input.get("age").getAsInt());
            delay.setInt(item,input.get("delay").getAsInt());
            var p = input.getAsJsonArray("p"); item.pos(new Vec3(p.get(0).getAsDouble(),p.get(1).getAsDouble(),p.get(2).getAsDouble()));
            if (world.server) item.setBoundingBox(new AABB(item.getX()-.125,item.getY(),item.getZ()-.125,item.getX()+.125,item.getY()+.25,item.getZ()+.125));
            var v = input.getAsJsonArray("v"); item.velocity = new Vec3(v.get(0).getAsDouble(),v.get(1).getAsDouble(),v.get(2).getAsDouble());
            item.grounded = input.get("ground").getAsBoolean();
            JsonArray ticks = new JsonArray();
            for (int i=0;i<40;i++) {
                item.tickCount++; item.tick();
                JsonObject tick = new JsonObject(); tick.add("p", vector(new Vec3(item.getX(),item.getY(),item.getZ())));
                tick.add("v",vector(item.velocity)); tick.addProperty("ground",item.grounded);
                tick.addProperty("fall",item.fallDistance);
                tick.addProperty("age",age.getInt(item)); tick.addProperty("delay",delay.getInt(item)); tick.addProperty("removed", item.removed); ticks.add(tick);
                if (item.removed) break;
            }
            output.add(ticks);
        }
        JsonArray merges = new JsonArray();
        for (int a=1;a<=64;a++) for (int b=1;b<=64;b++)
            merges.add(ItemEntity.areMergable(new ItemStack(Items.DIRT,a),new ItemStack(Items.DIRT,b)));
        output.add(merges);
        var tryMerge = ItemEntity.class.getDeclaredMethod("tryToMerge",ItemEntity.class); tryMerge.setAccessible(true);
        JsonArray pairResults = new JsonArray();
        for (int a=1;a<64;a++) for (int b=1;b<64;b++) {
            var first = (TestItem) unsafe.allocateInstance(TestItem.class);
            var second = (TestItem) unsafe.allocateInstance(TestItem.class);
            first.stack = new ItemStack(Items.DIRT,a); second.stack = new ItemStack(Items.DIRT,b);
            age.setInt(first,123); age.setInt(second,456); delay.setInt(first,7); delay.setInt(second,13);
            tryMerge.invoke(first,second);
            JsonArray result = new JsonArray();
            for (var entity : new TestItem[]{first, second}) {
                JsonArray row = new JsonArray(); row.add(entity.removed); row.add(entity.stack.getCount());
                row.add(age.getInt(entity)); row.add(delay.getInt(entity)); result.add(row);
            }
            pairResults.add(result);
        }
        output.add(pairResults);
        JsonArray tripleResults = new JsonArray();
        int[] amounts = {1,2,5,20,40};
        for (int a : amounts) for (int b : amounts) for (int c : amounts) {
            TestItem[] entities = new TestItem[3]; int[] counts = {a,b,c};
            for (int i=0;i<3;i++) {
                entities[i] = (TestItem) unsafe.allocateInstance(TestItem.class);
                entities[i].stack = new ItemStack(Items.DIRT,counts[i]);
                age.setInt(entities[i],100 + i*100); delay.setInt(entities[i],5 + i*5);
            }
            for (int i=1;i<3 && !entities[0].removed;i++) {
                if (!entities[i].removed && entities[i].stack.getCount()<64)
                    tryMerge.invoke(entities[0],entities[i]);
            }
            JsonArray result = new JsonArray();
            for (var entity : entities) {
                JsonArray row = new JsonArray(); row.add(entity.removed); row.add(entity.stack.getCount());
                row.add(age.getInt(entity)); row.add(delay.getInt(entity)); result.add(row);
            }
            tripleResults.add(result);
        }
        output.add(tripleResults);
        var storage = new EntitySectionStorage<Entity>(Entity.class, key -> Visibility.TICKING);
        TestItem[] sectionItems = new TestItem[9];
        for (int i=0;i<9;i++) {
            var entity = (TestItem) unsafe.allocateInstance(TestItem.class);
            sectionItems[i] = entity;
            entity.identity = i;
            double x = (i & 1) == 0 ? 0.05 : -0.05;
            double y = (i & 2) == 0 ? 0.05 : -0.05;
            double z = (i & 4) == 0 ? 0.05 : -0.05;
            entity.pos(new Vec3(x,y,z));
            entity.setBoundingBox(new AABB(x-.125,y,z-.125,x+.125,y+.25,z+.125));
            storage.getOrCreateSection(SectionPos.of((int)Math.floor(x/16), (int)Math.floor(y/16), (int)Math.floor(z/16)).asLong()).add(entity);
        }
        JsonArray sectionOrder = new JsonArray();
        storage.getEntities(new AABB(-1,-1,-1,1,1,1), entity -> {
            sectionOrder.add(((TestItem)entity).identity); return Continuation.CONTINUE;
        });
        output.add(sectionOrder);
        var section = storage.getOrCreateSection(SectionPos.of(0,0,0).asLong());
        section.remove(sectionItems[0]); section.add(sectionItems[0]);
        JsonArray afterReturn = new JsonArray();
        storage.getEntities(new AABB(-1,-1,-1,1,1,1), entity -> {
            afterReturn.add(((TestItem)entity).identity); return Continuation.CONTINUE;
        });
        output.add(afterReturn);
        JsonArray supportResults = new JsonArray();
        for (double half : new double[]{0.125,0.25,(double)0.3f}) for (int mode=0;mode<2;mode++) for (double x : new double[]{0.8,0.9,1.0,1.02,1.12,1.2}) {
            var world = (TestLevel)unsafe.allocateInstance(TestLevel.class);
            world.supportGrid=true; world.twoBlocks=mode==1;
            var item = (TestItem)unsafe.allocateInstance(TestItem.class); item.world=world;
            item.pos(new Vec3(x,1,0.5));
            var support = world.findSupportingBlock(item,new AABB(x-half,1-1.0e-6,0.5-half,x+half,1,0.5+half));
            supportResults.add(support.map(BlockPos::getX).orElse(-99));
        }
        output.add(supportResults);
        JsonArray livingBounce = new JsonArray();
        for (String name : new String[]{"minecraft:stone","minecraft:slime_block","minecraft:red_bed","minecraft:shelf_mushroom"})
        for (boolean suppress : new boolean[]{false,true})
        for (double entityBounce : new double[]{0.0,0.25,0.9})
        for (boolean ceiling : new boolean[]{false,true})
        for (double speed : new double[]{0.08,0.081,0.1,0.5,2.0})
        for (double ratio : new double[]{0,0.25,0.8}) {
            double velocity=ceiling ? speed : -speed;
            var world = (TestLevel)unsafe.allocateInstance(TestLevel.class);
            world.ground=net.minecraft.core.registries.BuiltInRegistries.BLOCK.getValue(net.minecraft.resources.Identifier.parse(name)).defaultBlockState();
            var player = (TestPlayer)unsafe.allocateInstance(TestPlayer.class);
            player.setWorld(world); player.velocity=new Vec3(0.3,velocity,-0.2); player.suppress=suppress; player.bounciness=entityBounce;
            player.verticalCollision=true; player.verticalCollisionBelow=!ceiling;
            RESTITUTE.invoke(player,world.ground,true,true,new Vec3(0,velocity*ratio,0));
            JsonObject row=new JsonObject(); row.addProperty("block",name); row.addProperty("suppress",suppress);
            row.addProperty("entityBounce",entityBounce); row.addProperty("ceiling",ceiling); row.addProperty("x",player.velocity.x); row.addProperty("z",player.velocity.z);
            row.addProperty("velocity",velocity); row.addProperty("ratio",ratio); row.addProperty("result",player.velocity.y);
            livingBounce.add(row);
        }
        output.add(livingBounce);
        Files.writeString(Path.of(args[1]),new Gson().toJson(output));
    }
}
"#;
        fn classpath(game: &Path, classes: &Path) -> OsString {
            let version = game.join("versions/26.3");
            let manifest: Value =
                serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
            let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
            for library in manifest["libraries"].as_array().unwrap() {
                if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                    let path = game.join("libraries").join(path);
                    if path.is_file() {
                        paths.push(path);
                    }
                }
            }
            std::env::join_paths(paths).unwrap()
        }
        fn run(command: &mut Command) {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "item oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fn vec(value: &Value) -> DVec3 {
            DVec3::new(
                value[0].as_f64().unwrap(),
                value[1].as_f64().unwrap(),
                value[2].as_f64().unwrap(),
            )
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("ItemMotionOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        let mut cases: Vec<_> = (0..64).map(|i| json!({"id":i, "p":[0.5, if i % 3 == 0 {0.0} else {2.0}, 0.5], "v":[(i as f64 % 5.0) * 0.012, if i % 4 == 0 {0.2} else {0.0}, -0.02], "ground":i%3==0, "ice":i%2==0, "block":(i/4)%4, "no_gravity":i%2==0, "age":([-32768,-2,0,42][i%4]), "delay":([32767,-1,0,3][(i/4)%4]), "fluid":i%3})).collect();
        for age in [5998, 5999, -32768] {
            cases.push(json!({"id":65, "p":[0.5,2.0,0.5], "v":[0.0,0.0,0.0], "ground":false, "ice":false, "block":0, "no_gravity":true, "age":age, "delay":32767, "fluid":0, "server":true}));
        }
        std::fs::write(&source, JAVA).unwrap();
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        let cp = classpath(&game, temp.path());
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("ItemMotionOracle")
            .arg(&input)
            .arg(&output));
        let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let merges = expected[cases.len()].as_array().unwrap();
        for a in 1..=64 {
            for b in 1..=64 {
                let left =
                    ItemStack::new("minecraft:dirt".into(), a, 64, Tag::Compound(vec![])).unwrap();
                let right =
                    ItemStack::new("minecraft:dirt".into(), b, 64, Tag::Compound(vec![])).unwrap();
                assert_eq!(
                    stacks_mergeable(&left, &right),
                    merges[((a - 1) * 64 + b - 1) as usize].as_bool().unwrap(),
                    "{a} + {b}"
                );
            }
        }
        let pairs = expected[cases.len() + 1].as_array().unwrap();
        for a in 1..64 {
            for b in 1..64 {
                let mut items = ItemEntities::default();
                for (count, lifetime, pickup_delay) in [(a, 123, 7), (b, 456, 13)] {
                    items.entities.push(ItemEntity {
                        item: DroppedItem {
                            entity_id: 0,
                            item_age: 0,
                            position: DVec3::ZERO,
                            previous_position: DVec3::ZERO,
                            stack: ItemStack::new(
                                "minecraft:dirt".into(),
                                count,
                                64,
                                Tag::Compound(vec![]),
                            )
                            .unwrap(),
                            age: 0,
                            bob_offset: 0.0,
                        },
                        velocity: DVec3::ZERO,
                        on_ground: false,
                        ground_without_support: false,
                        pickup_delay,
                        lifetime,
                        no_gravity: true,
                        owner: None,
                        _saved_source: None,
                        created: None,
                        id: 0,
                        section_sequence: 0,
                        random: LegacyRandom::new(0),
                        _damage_state: ItemDamageState::default(),
                    });
                }
                let mut removed = [false; 2];
                ItemEntities::merge_neighbours(&mut items.entities, &mut removed, &None, 0, None);
                let pair = &pairs[((a - 1) * 63 + b - 1) as usize];
                for (i, entity) in items.entities.iter().enumerate() {
                    assert_eq!(
                        removed[i],
                        pair[i][0].as_bool().unwrap(),
                        "{a} + {b} entity {i}"
                    );
                    if !removed[i] {
                        assert_eq!(
                            entity.item.stack.count(),
                            pair[i][1].as_u64().unwrap() as u32,
                            "{a} + {b}"
                        );
                        assert_eq!(
                            entity.lifetime,
                            pair[i][2].as_i64().unwrap() as i32,
                            "{a} + {b}"
                        );
                        assert_eq!(
                            entity.pickup_delay,
                            pair[i][3].as_i64().unwrap() as i32,
                            "{a} + {b}"
                        );
                    }
                }
            }
        }
        let triples = expected[cases.len() + 2].as_array().unwrap();
        let mut case_number = 0;
        for a in [1, 2, 5, 20, 40] {
            for b in [1, 2, 5, 20, 40] {
                for c in [1, 2, 5, 20, 40] {
                    let mut items = ItemEntities::default();
                    for (i, count) in [a, b, c].into_iter().enumerate() {
                        items.entities.push(ItemEntity {
                            item: DroppedItem {
                                entity_id: 0,
                                item_age: 0,
                                position: DVec3::ZERO,
                                previous_position: DVec3::ZERO,
                                stack: ItemStack::new(
                                    "minecraft:dirt".into(),
                                    count,
                                    64,
                                    Tag::Compound(vec![]),
                                )
                                .unwrap(),
                                age: 0,
                                bob_offset: 0.0,
                            },
                            velocity: DVec3::ZERO,
                            on_ground: false,
                            ground_without_support: false,
                            pickup_delay: 5 + i as i32 * 5,
                            lifetime: 100 + i as i32 * 100,
                            no_gravity: true,
                            owner: None,
                            _saved_source: None,
                            created: None,
                            id: i as u32,
                            section_sequence: 0,
                            random: LegacyRandom::new(0),
                            _damage_state: ItemDamageState::default(),
                        });
                    }
                    let mut removed = [false; 3];
                    ItemEntities::merge_neighbours(
                        &mut items.entities,
                        &mut removed,
                        &None,
                        0,
                        None,
                    );
                    for (i, entity) in items.entities.iter().enumerate() {
                        let expected = &triples[case_number][i];
                        assert_eq!(
                            removed[i],
                            expected[0].as_bool().unwrap(),
                            "{a}, {b}, {c}: entity {i}"
                        );
                        if !removed[i] {
                            assert_eq!(
                                entity.item.stack.count(),
                                expected[1].as_u64().unwrap() as u32
                            );
                            assert_eq!(entity.lifetime, expected[2].as_i64().unwrap() as i32);
                            assert_eq!(entity.pickup_delay, expected[3].as_i64().unwrap() as i32);
                        }
                    }
                    case_number += 1;
                }
            }
        }
        let mut order: Vec<_> = (0..9).collect();
        let position = |i| {
            DVec3::new(
                if i & 1 == 0 { 0.05 } else { -0.05 },
                if i & 2 == 0 { 0.05 } else { -0.05 },
                if i & 4 == 0 { 0.05 } else { -0.05 },
            )
        };
        order.sort_by_key(|i| (item_section_order(position(*i)), *i));
        let java_order: Vec<_> = expected[cases.len() + 3]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_u64().unwrap() as usize)
            .collect();
        assert_eq!(order, java_order);
        order.sort_by_key(|i| {
            (
                item_section_order(position(*i)),
                if *i == 0 { 9 } else { *i },
            )
        });
        let returned_order: Vec<_> = expected[cases.len() + 4]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_u64().unwrap() as usize)
            .collect();
        assert_eq!(order, returned_order);
        let support_results = expected[cases.len() + 5].as_array().unwrap();
        let mut support_index = 0;
        for half in [0.125, 0.25, f64::from(crate::PLAYER_HALF_WIDTH)] {
            for mode in 0..2 {
                for x in [0.8, 0.9, 1.0, 1.02, 1.12, 1.2] {
                    let support = movement::supporting_box(
                        DVec3::new(x, 1.0, 0.5),
                        half,
                        DVec3::ZERO,
                        true,
                        |pos| {
                            if pos.y == 0 && pos.z == 0 && (pos.x == 0 || mode == 1 && pos.x == 1) {
                                native_world::collision::unknown_barrier()
                            } else {
                                &[]
                            }
                        },
                    );
                    assert_eq!(
                        support.map_or(-99, |pos| pos.x),
                        support_results[support_index].as_i64().unwrap() as i32,
                        "support half={half}, mode={mode}, x={x}"
                    );
                    support_index += 1;
                }
            }
        }
        for row in expected[cases.len() + 6].as_array().unwrap() {
            let velocity = row["velocity"].as_f64().unwrap();
            let entity_bounce = if row["suppress"].as_bool().unwrap() {
                0.0
            } else {
                row["entityBounce"].as_f64().unwrap()
            };
            let restitution = if row["suppress"].as_bool().unwrap() {
                0.0
            } else {
                entity_bounce.max(f64::from(block_bounce_restitution(
                    row["block"].as_str().unwrap(),
                )))
            };
            assert!((row["x"].as_f64().unwrap() + 0.3 * entity_bounce).abs() < 1e-12);
            assert!((row["z"].as_f64().unwrap() - 0.2 * entity_bounce).abs() < 1e-12);
            let actual_y = velocity * row["ratio"].as_f64().unwrap();
            let result = if row["ceiling"].as_bool().unwrap() {
                collision_restitution(velocity, actual_y, entity_bounce, 0.08)
            } else {
                vertical_restitution(velocity, actual_y, restitution, 0.08)
            };
            assert!(
                (result - row["result"].as_f64().unwrap()).abs() < 1e-12,
                "living bounce {row}: {result}"
            );
        }
        for (index, case) in cases.iter().enumerate() {
            let mut items = ItemEntities::default();
            let stack =
                ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![])).unwrap();
            items.entities.push(ItemEntity {
                item: DroppedItem {
                    entity_id: 0,
                    item_age: 0,
                    position: vec(&case["p"]),
                    previous_position: vec(&case["p"]),
                    stack,
                    age: 0,
                    bob_offset: 0.0,
                },
                velocity: vec(&case["v"]),
                on_ground: case["ground"].as_bool().unwrap(),
                ground_without_support: false,
                pickup_delay: case["delay"].as_i64().unwrap() as i32,
                lifetime: case["age"].as_i64().unwrap() as i32,
                no_gravity: case["no_gravity"].as_bool().unwrap(),
                owner: None,
                _saved_source: None,
                created: Some(CreatedItemIdentity {
                    uuid: Some([1, 2, 3, 4]),
                    origin: vec(&case["p"]),
                    thrower: CreatedThrower::Known(None),
                    yaw: 0.0,
                    fall_distance: 0.0,
                    verified_position: vec(&case["p"]),
                    visited: created_chunk(vec(&case["p"])).into_iter().collect(),
                    base_valid: true,
                }),
                id: index as u32,
                section_sequence: 0,
                random: LegacyRandom::new(0),
                _damage_state: ItemDamageState::default(),
            });
            let mut inventory = Arc::new(PlayerInventory::default());
            let block = case["block"].as_u64().unwrap();
            let friction = if block == 1 {
                0.8
            } else if block == 0 && case["ice"].as_bool().unwrap() {
                0.98
            } else {
                0.6
            };
            let bounce = match block {
                1 => 1.0,
                2 | 3 => 0.75,
                _ => 0.0,
            };
            for (tick, oracle) in expected[index].as_array().unwrap().iter().enumerate() {
                items.tick_with_fluid_and_proof(
                    IVec3::ZERO,
                    DVec3::splat(1000.0),
                    PlayerPose::Standing,
                    GameMode::Survival,
                    &mut inventory,
                    |pos| {
                        if pos.y < 0 {
                            native_world::collision::unknown_barrier()
                        } else {
                            &[]
                        }
                    },
                    |_| friction,
                    |_| bounce,
                    |_| false,
                    |bounds| {
                        use native_world::fluid::{Cell, FluidKind, FluidState};
                        let kind = match case["fluid"].as_i64().unwrap() {
                            1 => Some(FluidKind::Water),
                            2 => Some(FluidKind::Lava),
                            _ => None,
                        };
                        native_world::fluid::contacts(
                            bounds,
                            bounds.min.y,
                            &|_| {
                                kind.map_or(Cell::Empty, |kind| {
                                    Cell::Fluid(FluidState {
                                        kind,
                                        amount: 8,
                                        source: true,
                                        falling: false,
                                    })
                                })
                            },
                            &|_, _| false,
                        )
                    },
                    false,
                    |_, _, _| None,
                    |_, _| true,
                    |_| Some(false),
                );
                if oracle["removed"].as_bool() == Some(true) {
                    assert!(
                        items.entities.is_empty(),
                        "Java expired item must be removed"
                    );
                    assert_eq!(
                        items.created_removal_effects(&BTreeSet::from([
                            native_world::loaded::ChunkPos { x: 0, z: 0 }
                        ])),
                        vec![([1, 2, 3, 4], CreatedRemovalCause::Expired)]
                    );
                    break;
                }
                let actual = &items.entities[0];
                let position_error = (actual.item.position - vec(&oracle["p"]))
                    .abs()
                    .max_element();
                let velocity_error = (actual.velocity - vec(&oracle["v"])).abs().max_element();
                assert!(
                    position_error < 1.0e-10
                        && velocity_error < 1.0e-10
                        && actual.on_ground == oracle["ground"].as_bool().unwrap()
                        && actual.lifetime == oracle["age"].as_i64().unwrap() as i32
                        && actual.pickup_delay == oracle["delay"].as_i64().unwrap() as i32
                        && (actual.created.as_ref().unwrap().fall_distance
                            - oracle["fall"].as_f64().unwrap())
                        .abs()
                            < 1e-12,
                    "case {index} tick {tick}: actual position {:?} velocity {:?} grounded {}, expected {oracle}",
                    actual.item.position,
                    actual.velocity,
                    actual.on_ground
                );
                if case["fluid"].as_i64() == Some(0) {
                    let group = actual.created.as_ref().unwrap().visited.clone();
                    let drafts = items.draft_created_items_in_chunks(&group).unwrap();
                    let saved = crate::saved_item::read(&drafts[0].document().value)
                        .unwrap()
                        .unwrap();
                    assert_eq!(saved.position, actual.item.position);
                    assert_eq!(saved.velocity, actual.velocity);
                    assert_eq!(i64::from(saved.age), oracle["age"].as_i64().unwrap());
                    assert_eq!(
                        i64::from(saved.pickup_delay),
                        oracle["delay"].as_i64().unwrap()
                    );
                    assert_eq!(
                        drafts[0].document().value.get("fall_distance"),
                        Some(&Tag::Double(oracle["fall"].as_f64().unwrap()))
                    );
                }
            }
        }
    }
}
