//! In-memory item drops for the currently supported basic block loot.
//!
//! Fluid movement, item-specific effects, entity saving and the
//! world's exact random stream are not implemented yet.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
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

#[derive(Clone, Debug)]
pub struct DroppedItem {
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

/// The prototype must be resolved from the installed item registry before a
/// missing component patch can be interpreted as non-resistant.
pub enum DamageResistancePrototype<'a> {
    Unknown,
    Absent,
    Component(&'a Tag),
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
        matches!(component, Tag::Compound(_)),
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
    let fire = Tag::Compound(vec![(
        "types".into(),
        Tag::String("#minecraft:is_fire".into()),
    )]);
    let prototype = match items.damage_resistance_prototype(stack.id()) {
        Builtin::FireTag => DamageResistancePrototype::Component(&fire),
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
    pickup_delay: i32,
    lifetime: i32,
    no_gravity: bool,
    owner: Option<[i32; 4]>,
    _saved_source: Option<Arc<Tag>>,
    id: u32,
    section_sequence: u64,
    _damage_state: ItemDamageState,
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
    random: LegacyRandom,
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
            random: LegacyRandom::new(0),
            next_id: 0,
            next_section_sequence: 0,
            player_uuid: None,
            active_chunks: None,
            damage_rules: None,
        }
    }
}

impl ItemEntities {
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

    pub(crate) fn validate_item_chunk_group(
        &self,
        positions: &BTreeSet<native_world::loaded::ChunkPos>,
        saved_uuids: &std::collections::BTreeMap<[i32; 4], native_world::loaded::ChunkPos>,
    ) -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
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
            if !positions.contains(&destination)
                && !source.is_some_and(|pos| positions.contains(&pos))
            {
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
        use anyhow::{Context, ensure};
        for entity in &self.entities {
            let Some(source) = &entity._saved_source else {
                continue;
            };
            if crate::saved_item::optional_uuid(source, "UUID")? != Some(uuid) {
                continue;
            }
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
            return Ok(Some(crate::saved_item::draft(&original, state)?));
        }
        Ok(None)
    }

    #[cfg(test)]
    pub(crate) fn debug_damage_states(&self) -> Vec<(i32, i32)> {
        self.entities
            .iter()
            .map(|item| (item._damage_state.health, item._damage_state.fire_ticks))
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
                let entity = &mut self.entities[index];
                // Java creates a fresh entity from NBT on chunk reload. Its
                // lifetime is saved, but Entity.tickCount is not.
                entity.item.age = 0;
                entity.item.previous_position = entity.item.position;
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
        let bob_offset = self.bob_offset();
        self.entities.push(ItemEntity {
            item: DroppedItem {
                position,
                previous_position: position,
                stack: saved.stack,
                age: 0,
                bob_offset,
            },
            velocity: saved.velocity,
            on_ground: saved.on_ground,
            pickup_delay: i32::from(saved.pickup_delay),
            lifetime: i32::from(saved.age),
            no_gravity: saved.no_gravity,
            owner: saved.owner,
            _saved_source: Some(saved.source),
            id: self.next_id,
            section_sequence: self.next_section_sequence,
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
        let velocity = DVec3::new(
            self.random.next_double() * 0.2 - 0.1,
            0.2,
            self.random.next_double() * 0.2 - 0.1,
        );
        let bob_offset = self.bob_offset();
        self.entities.push(ItemEntity {
            item: DroppedItem {
                position,
                previous_position: position,
                stack,
                age: 0,
                bob_offset,
            },
            velocity,
            on_ground: false,
            pickup_delay: 10,
            lifetime: 0,
            no_gravity: false,
            owner: None,
            _saved_source: None,
            id: self.next_id,
            section_sequence: self.next_section_sequence,
            _damage_state: ItemDamageState::default(),
        });
        self.next_id = self.next_id.wrapping_add(1);
        self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
    }

    fn bob_offset(&self) -> f32 {
        // Keep visual randomness independent of the incomplete loot/physics RNG.
        let mut seed = native_core::random::XoroshiroRandom::new(i64::from(self.next_id));
        let mut random = LegacyRandom::new(seed.next_long());
        random.next_float() * std::f32::consts::PI * 2.0
    }

    pub fn snapshot(&self) -> Arc<[DroppedItem]> {
        self.entities
            .iter()
            .filter(|entity| Self::active(&self.active_chunks, entity.item.position))
            .map(|entity| entity.item.clone())
            .collect()
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
        let bob_offset = self.bob_offset();
        self.entities.push(ItemEntity {
            item: DroppedItem {
                position,
                previous_position: position,
                stack,
                age: 0,
                bob_offset,
            },
            velocity,
            on_ground: false,
            pickup_delay: 40,
            lifetime: 0,
            no_gravity: false,
            owner: None,
            _saved_source: None,
            id: self.next_id,
            section_sequence: self.next_section_sequence,
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
        let bob_offset = self.bob_offset();
        self.entities.push(ItemEntity {
            item: DroppedItem {
                position,
                previous_position: position,
                stack,
                age: 0,
                bob_offset,
            },
            velocity,
            on_ground: false,
            pickup_delay: 40,
            lifetime: 0,
            no_gravity: false,
            owner: None,
            _saved_source: None,
            id: self.next_id,
            section_sequence: self.next_section_sequence,
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
        let Some(material) = Material::from_block_name(name) else {
            return;
        };
        if !enabled
            || mode == GameMode::Creative
            || !mining::mining_properties(material, hand)
                .is_some_and(|properties| properties.correct_for_drops)
        {
            return;
        }
        let Some(tables) = tables else { return };
        let tool = BasicTool {
            item: hand.map(ItemStack::id),
            count: hand.map_or(0, ItemStack::count),
        };
        let drops =
            match tables.drops_with_random(name, tool, Some(&mut || self.random.next_float())) {
                Ok(drops) => drops,
                Err(error) => {
                    eprintln!("unsupported loot for {name}: {error:#}");
                    return;
                }
            };
        self.spawn_drops(position, IVec3::ZERO, DropStrategy::FromMiddle, drops);
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
        };
        match tables.drops_with_random(name, tool, Some(&mut || self.random.next_float())) {
            Ok(drops) => self.spawn_drops(position, IVec3::ZERO, DropStrategy::FromMiddle, drops),
            Err(error) => eprintln!("unsupported loot for {name}: {error:#}"),
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
            self.spawn_drops(position, face, strategy, drops);
        }
    }

    fn spawn_drops(
        &mut self,
        position: IVec3,
        face: IVec3,
        strategy: DropStrategy,
        drops: Vec<Drop>,
    ) {
        for drop in drops {
            let id = format!("{}:{}", drop.item.namespace, drop.item.path);
            let maximum = ordinary_item_default_max(&id)
                .or_else(|| native_world::Block::from_item_id(&id).map(|_| 64));
            let mut remaining = drop.count;
            while remaining > 0 {
                let count = remaining.min(maximum.unwrap_or(99));
                let components = Tag::Compound(Vec::new());
                let stack = match maximum {
                    Some(max) => ItemStack::new(id.clone(), count, max, components),
                    None => ItemStack::unresolved(id.clone(), count, components),
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
                let bob_offset = self.bob_offset();
                self.entities.push(ItemEntity {
                    item: DroppedItem {
                        position: location,
                        previous_position: location,
                        stack,
                        age: 0,
                        bob_offset,
                    },
                    velocity,
                    on_ground: false,
                    pickup_delay: 10,
                    lifetime: 0,
                    no_gravity: false,
                    owner: None,
                    _saved_source: None,
                    id: self.next_id,
                    section_sequence: self.next_section_sequence,
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
            512,
            |_| native_world::fluid::FluidContacts::default(),
            false,
            |_, _, _| None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tick_with_fluid<'a>(
        &mut self,
        origin: IVec3,
        player: DVec3,
        pose: PlayerPose,
        mode: GameMode,
        inventory: &mut Arc<PlayerInventory>,
        shape: impl FnMut(IVec3) -> &'a [Aabb],
        friction: impl FnMut(IVec3) -> f32,
        fluid: impl FnMut(BoundingBox) -> native_world::fluid::FluidContacts,
        fast_lava: bool,
        path: impl FnMut(BoundingBox, DVec3, DVec3) -> Option<Vec<native_world::fluid::FluidKind>>,
    ) {
        self.tick_with_threshold(
            origin, player, pose, mode, inventory, shape, friction, 512, fluid, fast_lava, path,
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
        index_threshold: usize,
        mut fluid: impl FnMut(BoundingBox) -> native_world::fluid::FluidContacts,
        fast_lava: bool,
        mut path: impl FnMut(BoundingBox, DVec3, DVec3) -> Option<Vec<native_world::fluid::FluidKind>>,
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
                if hit_x {
                    entity.velocity.x = 0.0;
                }
                if hit_z {
                    entity.velocity.z = 0.0;
                }
                if hit_y {
                    entity.velocity.y = 0.0;
                }
                let drag = 0.98_f32;
                let ground_drag = if entity.on_ground {
                    let below = (entity.item.position
                        - origin.as_dvec3()
                        - DVec3::new(0.0, f64::from(0.999999_f32), 0.0))
                    .floor()
                    .as_ivec3();
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
            if removed[index] {
                continue;
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
                Self::merge_neighbours(
                    &mut self.entities,
                    &mut removed,
                    active_chunks,
                    index,
                    candidates.as_deref(),
                );
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
                Arc::make_mut(inventory).pickup_stack(&mut stack, mode == GameMode::Creative);
                if let Some(remainder) = stack {
                    entity.item.stack = remainder;
                } else {
                    removed[index] = true;
                    continue;
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
    ) {
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
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::packs::ResourceProvider;

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
            ("minecraft:nether_star", None),
            ("example:unknown", None),
        ] {
            let stack = ItemStack::new(id.into(), 1, 64, Tag::Compound(vec![])).unwrap();
            assert_eq!(
                damage_resistant_in_world(&stack, &items, &tags, "minecraft:lava").unwrap(),
                expected,
                "{id}"
            );
        }
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
                position: DVec3::ZERO,
                previous_position: DVec3::ZERO,
                stack: ItemStack::new(id.into(), 1, 64, Tag::Compound(vec![])).unwrap(),
                age: 0,
                bob_offset: 0.0,
            },
            velocity: DVec3::ZERO,
            on_ground: false,
            pickup_delay: 0,
            lifetime: 0,
            no_gravity: false,
            owner: None,
            _saved_source: None,
            id: 0,
            section_sequence: 0,
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
        let mut unknown = make("minecraft:nether_star");
        assert!(!apply_lava_contact(&mut unknown, &rules).unwrap());
        assert_eq!(unknown._damage_state, ItemDamageState::default());

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
        run(std::process::Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("ItemLavaOracle")
            .arg(&result)
            .arg(&fire_result));
        let oracle: serde_json::Value =
            serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        for mode in 0..3 {
            let mut item = make(if mode == 1 {
                "minecraft:netherite_ingot"
            } else {
                "minecraft:stone"
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
        let fire_oracle: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fire_result).unwrap()).unwrap();
        for mode in 0..3 {
            let mut item = make(if mode == 1 {
                "minecraft:netherite_ingot"
            } else {
                "minecraft:stone"
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
                            pickup_delay: 0,
                            lifetime: 0,
                            no_gravity: true,
                            owner: None,
                            _saved_source: None,
                            id: index as u32,
                            section_sequence: 0,
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
                                threshold,
                                |_| native_world::fluid::FluidContacts::default(),
                                false,
                                |_, _, _| None,
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
                pickup_delay: 0,
                lifetime: 0,
                no_gravity: true,
                owner: None,
                _saved_source: None,
                id: index as u32,
                section_sequence: 0,
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
                pickup_delay: 0,
                lifetime: 0,
                no_gravity: true,
                owner: None,
                _saved_source: None,
                id: index as u32,
                section_sequence: 0,
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
                pickup_delay: delay,
                lifetime,
                no_gravity: true,
                owner: None,
                _saved_source: None,
                id: count,
                section_sequence: 0,
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
                    position: DVec3::ZERO,
                    previous_position: DVec3::ZERO,
                    stack: ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![]))
                        .unwrap(),
                    age: 0,
                    bob_offset: 0.0,
                },
                velocity: DVec3::ZERO,
                on_ground: false,
                pickup_delay: 0,
                lifetime: 0,
                no_gravity: true,
                owner: None,
                _saved_source: None,
                id: 0,
                section_sequence: 0,
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
                pickup_delay: 0,
                lifetime: 0,
                no_gravity: true,
                owner: None,
                _saved_source: None,
                id: index as u32,
                section_sequence: index as u64,
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
                position,
                previous_position: position - DVec3::X,
                stack: ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![]))
                    .unwrap(),
                age: 39,
                bob_offset: 0.0,
            },
            velocity: DVec3::ZERO,
            on_ground: false,
            pickup_delay: 7,
            lifetime: 123,
            no_gravity: true,
            owner: None,
            _saved_source: None,
            id: 0,
            section_sequence: 4,
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
                position: DVec3::ZERO,
                previous_position: DVec3::ZERO,
                stack,
                age: 0,
                bob_offset: 0.0,
            },
            velocity: DVec3::ZERO,
            on_ground: false,
            pickup_delay: i32::from(i16::MAX),
            lifetime: i32::from(i16::MIN),
            no_gravity: true,
            owner: None,
            _saved_source: None,
            id: 0,
            section_sequence: 0,
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
                position: DVec3::ZERO,
                previous_position: DVec3::ZERO,
                stack,
                age: 0,
                bob_offset: 0.0,
            },
            velocity: DVec3::ZERO,
            on_ground: false,
            pickup_delay: 0,
            lifetime: 5799,
            no_gravity: true,
            owner: Some([1, 2, 3, 4]),
            _saved_source: None,
            id: 0,
            section_sequence: 0,
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
        items.drop_player(stack, IVec3::ZERO, player.position.as_dvec3(), player);
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
        items.drop_player(stack.clone(), IVec3::ZERO, DVec3::ZERO, player);
        items.drop_player(stack, IVec3::ZERO, DVec3::ZERO, player);
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
      var item = player.createItemStackToDrop(new ItemStack(Items.DIRT),false,false);
      var result = new JsonObject(); result.add("position",vec(new Vec3(item.getX(),item.getY(),item.getZ())));
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
            cases.push(json!({"seed":index as i64 + 31, "yaw":player.yaw.to_degrees()-180.0, "pitch":-player.pitch.to_degrees(), "eyeHeight":f64::from(pose.eye_height()), "position":player.position.to_array()}));
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
            items.drop_player(stack, IVec3::ZERO, player.position.as_dvec3(), player);
            let entity = &items.entities[0];
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
    static { try { POSITION = Entity.class.getDeclaredField("position"); POSITION.setAccessible(true); }
             catch (Exception e) { throw new RuntimeException(e); } }
    static class TestLevel extends ClientLevel {
        BlockState ground;
        TestLevel() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public boolean isClientSide() { return true; }
        @Override public BlockState getBlockState(BlockPos pos) { return ground; }
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
        @Override public void baseTick() { }
        @Override public boolean isInWater() { return fluidKind == 1; }
        @Override public boolean isInLava() { return fluidKind == 2; }
        @Override public double getFluidHeight(TagKey<Fluid> tag) { return 1.0; }
        @Override public boolean isNoGravity() { return noGravity; }
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
            if (blocked) velocity = new Vec3(velocity.x, 0, velocity.z);
        }
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
            world.ground = (input.get("ice").getAsBoolean() ? Blocks.ICE : Blocks.STONE).defaultBlockState();
            var item = (TestItem) unsafe.allocateInstance(TestItem.class);
            item.world = world; item.identity = input.get("id").getAsInt();
            item.noGravity = input.get("no_gravity").getAsBoolean();
            item.fluidKind = input.get("fluid").getAsInt();
            age.setInt(item,input.get("age").getAsInt());
            delay.setInt(item,input.get("delay").getAsInt());
            var p = input.getAsJsonArray("p"); item.pos(new Vec3(p.get(0).getAsDouble(),p.get(1).getAsDouble(),p.get(2).getAsDouble()));
            var v = input.getAsJsonArray("v"); item.velocity = new Vec3(v.get(0).getAsDouble(),v.get(1).getAsDouble(),v.get(2).getAsDouble());
            item.grounded = input.get("ground").getAsBoolean();
            JsonArray ticks = new JsonArray();
            for (int i=0;i<40;i++) {
                item.tickCount++; item.tick();
                JsonObject tick = new JsonObject(); tick.add("p", vector(new Vec3(item.getX(),item.getY(),item.getZ())));
                tick.add("v",vector(item.velocity)); tick.addProperty("ground",item.grounded);
                tick.addProperty("age",age.getInt(item)); tick.addProperty("delay",delay.getInt(item)); ticks.add(tick);
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
        let cases: Vec<_> = (0..32).map(|i| json!({"id":i, "p":[0.5, if i % 3 == 0 {0.0} else {2.0}, 0.5], "v":[(i as f64 % 5.0) * 0.012, if i % 4 == 0 {0.2} else {0.0}, -0.02], "ground":i%3==0, "ice":i%2==0, "no_gravity":i%2==0, "age":([-32768,-2,0,42][i%4]), "delay":([32767,-1,0,3][(i/4)%4]), "fluid":i%3})).collect();
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
                        pickup_delay,
                        lifetime,
                        no_gravity: true,
                        owner: None,
                        _saved_source: None,
                        id: 0,
                        section_sequence: 0,
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
                            pickup_delay: 5 + i as i32 * 5,
                            lifetime: 100 + i as i32 * 100,
                            no_gravity: true,
                            owner: None,
                            _saved_source: None,
                            id: i as u32,
                            section_sequence: 0,
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
        for (index, case) in cases.iter().enumerate() {
            let mut items = ItemEntities::default();
            let stack =
                ItemStack::new("minecraft:dirt".into(), 1, 64, Tag::Compound(vec![])).unwrap();
            items.entities.push(ItemEntity {
                item: DroppedItem {
                    position: vec(&case["p"]),
                    previous_position: vec(&case["p"]),
                    stack,
                    age: 0,
                    bob_offset: 0.0,
                },
                velocity: vec(&case["v"]),
                on_ground: case["ground"].as_bool().unwrap(),
                pickup_delay: case["delay"].as_i64().unwrap() as i32,
                lifetime: case["age"].as_i64().unwrap() as i32,
                no_gravity: case["no_gravity"].as_bool().unwrap(),
                owner: None,
                _saved_source: None,
                id: index as u32,
                section_sequence: 0,
                _damage_state: ItemDamageState::default(),
            });
            let mut inventory = Arc::new(PlayerInventory::default());
            let friction = if case["ice"].as_bool().unwrap() {
                0.98
            } else {
                0.6
            };
            for (tick, oracle) in expected[index].as_array().unwrap().iter().enumerate() {
                items.tick_with_fluid(
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
                );
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
                        && actual.pickup_delay == oracle["delay"].as_i64().unwrap() as i32,
                    "case {index} tick {tick}: actual position {:?} velocity {:?} grounded {}, expected {oracle}",
                    actual.item.position,
                    actual.velocity,
                    actual.on_ground
                );
            }
        }
    }
}
