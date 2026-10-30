//! Experience orbs from player deaths and supported saved entities. Entity IDs
//! and the world's exact RNG are not yet reproduced.
//! Pickup is withheld when equipment repair effects cannot be resolved.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use glam::{DVec3, IVec3};
use native_core::random::LegacyRandom;
use native_resources::enchantments::OxygenEnchantments;
use native_world::{collision::Aabb, movement::BoundingBox};

use crate::{GameMode, PlayerPose, experience::Experience, inventory::PlayerInventory, movement};

const VALUES: [i32; 11] = [1, 3, 7, 17, 37, 73, 149, 307, 617, 1237, 2477];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExperienceOrb {
    /// Absolute coordinates keep orbs stable when the loaded window shifts.
    pub position: DVec3,
    pub previous_position: DVec3,
    pub value: i32,
    pub age: u32,
}

impl ExperienceOrb {
    pub fn icon(self) -> usize {
        VALUES
            .iter()
            .rposition(|value| self.value >= *value)
            .unwrap_or(0)
    }
}

#[derive(Clone)]
struct Orb {
    id: i32,
    section_sequence: u64,
    visual: ExperienceOrb,
    velocity: DVec3,
    on_ground: bool,
    ground_without_support: bool,
    no_gravity: bool,
    lifetime: i32,
    count: i32,
    random: LegacyRandom,
    source: Option<Arc<native_persistence::nbt::Tag>>,
    save_unresolved: bool,
    damage: OrbDamageState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OrbDamageState {
    health: i32,
    fire_ticks: i32,
    invulnerable: bool,
    invulnerable_time: i32,
}

impl Default for OrbDamageState {
    fn default() -> Self {
        Self {
            health: 5,
            fire_ticks: 0,
            invulnerable: false,
            invulnerable_time: 0,
        }
    }
}

pub(crate) struct Orbs {
    entities: Vec<Orb>,
    snapshot: Arc<[ExperienceOrb]>,
    random: LegacyRandom,
    entity_seeder: LegacyRandom,
    award_random: LegacyRandom,
    pickup_delay: u8,
    next_entity_id: i32,
    next_section_sequence: u64,
    active_chunks: Option<BTreeSet<native_world::loaded::ChunkPos>>,
    damage_tags: Option<Arc<OxygenEnchantments>>,
}

impl Default for Orbs {
    fn default() -> Self {
        Self {
            entities: Vec::new(),
            snapshot: Arc::from([]),
            random: LegacyRandom::new(0),
            entity_seeder: LegacyRandom::new(1),
            award_random: LegacyRandom::new(2),
            pickup_delay: 0,
            next_entity_id: 1,
            next_section_sequence: 0,
            active_chunks: None,
            damage_tags: None,
        }
    }
}

impl Orbs {
    pub(crate) fn push_from_soil_expansion(&mut self, block: IVec3) {
        let mut changed = false;
        for entity in &mut self.entities {
            let before = entity.visual.position;
            if !Self::active(&self.active_chunks, before) {
                continue;
            }
            let Some(delta) =
                native_world::movement::soil_expansion_push(orb_bounds(before), block)
            else {
                continue;
            };
            entity.visual.position.y += delta;
            entity.visual.previous_position = entity.visual.position;
            if crate::items::item_section_order(before)
                != crate::items::item_section_order(entity.visual.position)
            {
                entity.section_sequence = self.next_section_sequence;
                self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
            }
            changed = true;
        }
        if changed {
            self.update_snapshot();
        }
    }

    pub fn snapshot(&self) -> Arc<[ExperienceOrb]> {
        Arc::clone(&self.snapshot)
    }

    pub fn reset_player_pickup_delay(&mut self) {
        self.pickup_delay = 0;
    }

    pub fn set_damage_tags(&mut self, tags: Arc<OxygenEnchantments>) {
        self.damage_tags = Some(tags);
    }

    fn update_snapshot(&mut self) {
        self.snapshot = self
            .entities
            .iter()
            .filter(|entity| Self::active(&self.active_chunks, entity.visual.position))
            .map(|entity| entity.visual)
            .collect();
    }

    fn active(chunks: &Option<BTreeSet<native_world::loaded::ChunkPos>>, position: DVec3) -> bool {
        chunks
            .as_ref()
            .is_none_or(|chunks| crate::items::ItemEntities::active_in(chunks, position))
    }

    pub fn set_active_chunks(&mut self, chunks: BTreeSet<native_world::loaded::ChunkPos>) {
        if self.active_chunks.is_some() {
            let mut restored: Vec<_> = self
                .entities
                .iter()
                .enumerate()
                .filter(|(_, entity)| {
                    !Self::active(&self.active_chunks, entity.visual.position)
                        && crate::items::ItemEntities::active_in(&chunks, entity.visual.position)
                })
                .map(|(index, entity)| (entity.section_sequence, index))
                .collect();
            restored.sort_unstable();
            for (_, index) in restored {
                let entity = &mut self.entities[index];
                entity.visual.age = 0;
                entity.visual.previous_position = entity.visual.position;
                entity.ground_without_support = false;
                entity.id = self.next_entity_id;
                self.next_entity_id = self.next_entity_id.wrapping_add(1);
                entity.section_sequence = self.next_section_sequence;
                self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
            }
        }
        self.active_chunks = Some(chunks);
        self.update_snapshot();
    }

    pub fn spawn_saved(&mut self, saved: crate::saved_orb::SavedOrb) {
        let id = self.next_entity_id;
        self.next_entity_id = self.next_entity_id.wrapping_add(1);
        let section_sequence = self.next_section_sequence;
        self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
        self.entities.push(Orb {
            id,
            section_sequence,
            visual: ExperienceOrb {
                position: saved.position,
                previous_position: saved.position,
                value: saved.value,
                age: 0,
            },
            velocity: saved.velocity,
            on_ground: saved.on_ground,
            ground_without_support: false,
            no_gravity: saved.no_gravity,
            lifetime: saved.age,
            count: saved.count,
            random: LegacyRandom::new(self.entity_seeder.next_long()),
            source: Some(saved.source),
            save_unresolved: false,
            damage: OrbDamageState {
                health: saved.health,
                fire_ticks: i32::from(saved.fire_ticks),
                invulnerable: saved.invulnerable,
                invulnerable_time: saved.invulnerable_time,
            },
        });
        self.update_snapshot();
    }

    pub fn has_in_chunk(&self, pos: native_world::loaded::ChunkPos) -> bool {
        let mut chunk = BTreeSet::new();
        chunk.insert(pos);
        self.entities
            .iter()
            .any(|entity| crate::items::ItemEntities::active_in(&chunk, entity.visual.position))
    }

    pub fn draft_saved(
        &self,
        uuid: [i32; 4],
    ) -> anyhow::Result<Option<crate::saved_orb::OrbDraft>> {
        for orb in &self.entities {
            let Some(source) = &orb.source else {
                continue;
            };
            if crate::saved_item::optional_uuid(source, "UUID")? != Some(uuid) {
                continue;
            }
            anyhow::ensure!(
                !orb.save_unresolved,
                "experience-orb damage effects are not simulated"
            );
            let original = native_persistence::nbt::NamedTag {
                name: "".into(),
                value: (**source).clone(),
            };
            let state = crate::saved_orb::OrbSaveState {
                position: orb.visual.position,
                velocity: orb.velocity,
                on_ground: orb.on_ground,
                no_gravity: orb.no_gravity,
                age: orb.lifetime,
                value: orb.visual.value,
                count: orb.count,
                health: orb.damage.health,
                fire_ticks: orb.damage.fire_ticks,
                invulnerable: orb.damage.invulnerable,
                invulnerable_time: orb.damage.invulnerable_time,
            };
            return Ok(Some(crate::saved_orb::draft(&original, state)?));
        }
        Ok(None)
    }

    pub fn validate_chunk_group(
        &self,
        positions: &BTreeSet<native_world::loaded::ChunkPos>,
        saved_uuids: &std::collections::BTreeMap<[i32; 4], native_world::loaded::ChunkPos>,
    ) -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        let mut seen = BTreeSet::new();
        for orb in &self.entities {
            let position = orb.visual.position;
            ensure!(
                position.is_finite()
                    && position.x.abs() <= 30_000_512.0
                    && position.z.abs() <= 30_000_512.0,
                "experience orb has an invalid position"
            );
            let destination = native_world::loaded::ChunkPos {
                x: (position.x.floor() as i32).div_euclid(16),
                z: (position.z.floor() as i32).div_euclid(16),
            };
            let uuid = orb
                .source
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
            let uuid = uuid.context("new experience orb needs coordinated creation")?;
            ensure!(seen.insert(uuid), "duplicate live experience-orb UUID");
            ensure!(
                source.is_some_and(|pos| positions.contains(&pos)),
                "experience-orb source chunk is unavailable"
            );
            ensure!(
                positions.contains(&destination),
                "experience-orb destination chunk is unavailable"
            );
        }
        Ok(())
    }

    pub fn drop_player_experience(
        &mut self,
        position: DVec3,
        xp: Experience,
        keep: bool,
        mode: GameMode,
    ) {
        if keep || mode == GameMode::Spectator {
            return;
        }
        let mut amount = xp.level.wrapping_mul(7).min(100);
        while amount > 0 {
            let value = VALUES
                .iter()
                .rev()
                .copied()
                .find(|value| *value <= amount)
                .unwrap_or(1);
            amount -= value;
            if try_merge_award(
                &mut self.entities,
                &self.active_chunks,
                position,
                value,
                &mut self.award_random,
            ) {
                continue;
            }
            // The Java constructor also consumes a random yaw before its
            // three velocity components. This session has its own RNG.
            self.random.next_float();
            let velocity = DVec3::new(
                (self.random.next_double() * 0.2 - 0.1) * 2.0,
                self.random.next_double() * 0.2 * 2.0,
                (self.random.next_double() * 0.2 - 0.1) * 2.0,
            );
            let id = self.next_entity_id;
            self.next_entity_id = self.next_entity_id.wrapping_add(1);
            let section_sequence = self.next_section_sequence;
            self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
            self.entities.push(Orb {
                id,
                section_sequence,
                visual: ExperienceOrb {
                    position,
                    previous_position: position,
                    value,
                    age: 0,
                },
                velocity,
                on_ground: false,
                ground_without_support: false,
                no_gravity: false,
                lifetime: 0,
                count: 1,
                random: LegacyRandom::new(self.entity_seeder.next_long()),
                source: None,
                save_unresolved: false,
                damage: OrbDamageState::default(),
            });
        }
        if !self.entities.is_empty() {
            self.update_snapshot();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tick<'a>(
        &mut self,
        origin: IVec3,
        player: DVec3,
        pose: PlayerPose,
        mode: GameMode,
        alive: bool,
        inventory: &mut Arc<PlayerInventory>,
        player_random: &mut LegacyRandom,
        xp: &mut Experience,
        score: &mut i32,
        shape: impl FnMut(IVec3) -> &'a [Aabb],
        friction: impl FnMut(IVec3) -> f32,
    ) {
        self.tick_with_fluid(
            origin,
            player,
            pose,
            mode,
            alive,
            inventory,
            player_random,
            xp,
            score,
            shape,
            friction,
            |_| false,
            |_, _| (Default::default(), false),
            false,
            |_, _, _| Some(Vec::new()),
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tick_with_fluid<'a>(
        &mut self,
        origin: IVec3,
        player: DVec3,
        pose: PlayerPose,
        mode: GameMode,
        alive: bool,
        inventory: &mut Arc<PlayerInventory>,
        player_random: &mut LegacyRandom,
        xp: &mut Experience,
        score: &mut i32,
        mut shape: impl FnMut(IVec3) -> &'a [Aabb],
        mut friction: impl FnMut(IVec3) -> f32,
        mut keep_support_height: impl FnMut(IVec3) -> bool,
        mut fluid: impl FnMut(BoundingBox, DVec3) -> (native_world::fluid::FluidContacts, bool),
        fast_lava: bool,
        mut path: impl FnMut(BoundingBox, DVec3, DVec3) -> Option<Vec<native_world::fluid::FluidKind>>,
    ) {
        self.pickup_delay = self.pickup_delay.saturating_sub(1);
        if self.entities.is_empty() {
            return;
        }
        let player_box = movement::player_bounds(player, pose.height());
        let pickup = BoundingBox {
            min: player_box.min - DVec3::new(1.0, 0.5, 1.0),
            max: player_box.max + DVec3::new(1.0, 0.5, 1.0),
        };
        let can_pickup = alive && mode != GameMode::Spectator;
        let active_chunks = &self.active_chunks;
        let damage_tags = self.damage_tags.as_deref();
        let mut removed = vec![false; self.entities.len()];
        let mut spatial = None;
        for index in 0..self.entities.len() {
            if removed[index] || !Self::active(active_chunks, self.entities[index].visual.position)
            {
                continue;
            }
            let entity = &mut self.entities[index];
            let relative = entity.visual.position - origin.as_dvec3();
            let bounds = orb_bounds(relative);
            entity.visual.previous_position = entity.visual.position;
            let (contacts, lava_at_position) = fluid(bounds, relative);
            if entity.damage.invulnerable_time > 0 {
                entity.damage.invulnerable_time -= 1;
            }
            if entity.damage.fire_ticks > 0 && damage_tags.is_none() {
                entity.save_unresolved = true;
            }
            match orb_fire_tick(&mut entity.damage, contacts.lava.depth > 0.0, damage_tags) {
                Some(true) => {
                    removed[index] = true;
                    continue;
                }
                Some(false) => {}
                None => {
                    entity.save_unresolved = true;
                }
            }
            if contacts.lava.depth > 0.0 || lava_at_position {
                entity.save_unresolved = true;
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
            if contacts.water.eyes_inside {
                entity.velocity = underwater_velocity(entity.velocity);
            } else if !entity.no_gravity && movement::no_collision(bounds, &mut shape) {
                entity.velocity.y -= 0.03;
            }
            if lava_at_position {
                entity.velocity = lava_bounce(&mut entity.random);
            }
            let next_tick = entity.visual.age.wrapping_add(1);
            if (next_tick as i32) % 20 == 1 {
                if spatial.is_none() && self.entities.len() >= 512 {
                    let mut cells = HashMap::<(i64, i64, i64), Vec<usize>>::new();
                    for (other, entity) in self.entities.iter().enumerate() {
                        if !removed[other] && Self::active(active_chunks, entity.visual.position) {
                            cells
                                .entry(orb_cell(entity.visual.position))
                                .or_default()
                                .push(other);
                        }
                    }
                    spatial = Some(cells);
                }
                let candidates = spatial
                    .as_ref()
                    .map(|cells| orb_candidates(cells, self.entities[index].visual.position));
                merge_nearby(
                    &mut self.entities,
                    &mut removed,
                    active_chunks,
                    index,
                    candidates.as_deref(),
                );
            }
            let entity = &mut self.entities[index];
            if alive && mode != GameMode::Spectator && (player - relative).length_squared() < 64.0 {
                entity.velocity += attraction(relative, player, pose);
            }
            let requested = entity.velocity;
            let actual =
                movement::resolve_box(bounds, requested, entity.on_ground, 0.0, &mut shape);
            entity.visual.position += actual;
            match path(bounds, actual, requested) {
                Some(effects) => {
                    for effect in effects {
                        match effect {
                            native_world::fluid::FluidKind::Lava => {
                                entity.save_unresolved = true;
                                match orb_lava_contact(&mut entity.damage, damage_tags) {
                                    Some(true) => {
                                        removed[index] = true;
                                        break;
                                    }
                                    Some(false) => {}
                                    None => {
                                        entity.save_unresolved = true;
                                    }
                                }
                            }
                            native_world::fluid::FluidKind::Water => {
                                entity.damage.fire_ticks = entity.damage.fire_ticks.min(0)
                            }
                        }
                    }
                }
                None => {
                    entity.save_unresolved = true;
                }
            }
            if removed[index] {
                continue;
            }
            if crate::items::item_section_order(entity.visual.previous_position)
                != crate::items::item_section_order(entity.visual.position)
            {
                entity.section_sequence = self.next_section_sequence;
                self.next_section_sequence = self.next_section_sequence.wrapping_add(1);
            }
            if let Some(cells) = &mut spatial {
                let previous = orb_cell(entity.visual.previous_position);
                let current = orb_cell(entity.visual.position);
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
            if (requested.x - actual.x).abs() >= f64::from(1.0e-5_f32) {
                entity.velocity.x = 0.0;
            }
            if (requested.z - actual.z).abs() >= f64::from(1.0e-5_f32) {
                entity.velocity.z = 0.0;
            }
            let hit_y = requested.y != actual.y;
            entity.on_ground = hit_y && requested.y < 0.0;
            let relative_position = entity.visual.position - origin.as_dvec3();
            let support = if entity.on_ground {
                let found = movement::supporting_box(
                    relative_position,
                    0.25,
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
            if hit_y {
                entity.velocity.y = 0.0;
            }
            let mut drag = 0.98_f32;
            if entity.on_ground {
                let below = movement::movement_on_block(
                    relative_position,
                    support,
                    &mut keep_support_height,
                );
                drag *= friction(below);
            }
            entity.velocity *= f64::from(drag);
            if entity.on_ground && requested.y < -0.03 {
                entity.velocity.y = -requested.y * 0.4;
            }
            entity.visual.age = next_tick;
            entity.lifetime = entity.lifetime.wrapping_add(1);
            if entity.lifetime >= 6000 {
                removed[index] = true;
                continue;
            }
            let bounds = orb_bounds(entity.visual.position - origin.as_dvec3());
            if self.pickup_delay == 0
                && can_pickup
                && bounds.max.cmpgt(pickup.min).all()
                && bounds.min.cmplt(pickup.max).all()
            {
                let mut random = player_random.clone();
                if let Ok((remaining, repaired)) =
                    inventory.repair_with_experience(entity.visual.value, &mut random)
                {
                    let mut next_xp = *xp;
                    let mut next_score = *score;
                    if remaining <= 0 || next_xp.give_points(&mut next_score, remaining) {
                        if let Some(repaired) = repaired {
                            *Arc::make_mut(inventory) = repaired;
                        }
                        *xp = next_xp;
                        *score = next_score;
                        *player_random = random;
                        self.pickup_delay = 2;
                        entity.count = entity.count.wrapping_sub(1);
                        if entity.count == 0 {
                            removed[index] = true;
                        }
                        continue;
                    }
                }
            }
        }
        let mut index = 0;
        self.entities.retain(|_| {
            let keep = !removed[index];
            index += 1;
            keep
        });
        self.update_snapshot();
    }
}

fn orb_hurt(
    state: &mut OrbDamageState,
    amount: f32,
    kind: &str,
    tags: Option<&OxygenEnchantments>,
) -> Option<bool> {
    if let Some(tags) = tags {
        if !tags.has_damage_type(kind).ok()? {
            return None;
        }
        if tags.damage_type_has_tag("minecraft:is_fall", kind).ok()?
            && tags.fall_damage_immune("minecraft:experience_orb").ok()?
        {
            return Some(false);
        }
    }
    if state.invulnerable || state.invulnerable_time > 0 {
        let bypass = tags?
            .damage_type_has_tag("minecraft:bypasses_invulnerability", kind)
            .ok()?;
        if !bypass {
            return Some(false);
        }
    }
    state.health = (state.health as f32 - amount) as i32;
    Some(state.health <= 0)
}

fn orb_lava_contact(state: &mut OrbDamageState, tags: Option<&OxygenEnchantments>) -> Option<bool> {
    state.fire_ticks = state.fire_ticks.max(300);
    orb_hurt(state, 4.0, "minecraft:lava", tags)
}

fn orb_fire_tick(
    state: &mut OrbDamageState,
    in_lava: bool,
    tags: Option<&OxygenEnchantments>,
) -> Option<bool> {
    if state.fire_ticks <= 0 {
        return Some(false);
    }
    let damage = if state.fire_ticks % 20 == 0 && !in_lava {
        orb_hurt(state, 1.0, "minecraft:on_fire", tags)
    } else {
        Some(false)
    };
    state.fire_ticks -= 1;
    damage
}

fn try_merge_award(
    entities: &mut [Orb],
    active: &Option<BTreeSet<native_world::loaded::ChunkPos>>,
    position: DVec3,
    value: i32,
    random: &mut LegacyRandom,
) -> bool {
    let id = random.next_int_bound(40).expect("positive bound");
    let query = BoundingBox {
        min: position - DVec3::splat(0.5),
        max: position + DVec3::splat(0.5),
    };
    let first = entities
        .iter()
        .enumerate()
        .filter(|(_, orb)| {
            if !Orbs::active(active, orb.visual.position)
                || orb.id.wrapping_sub(id) % 40 != 0
                || orb.visual.value != value
            {
                return false;
            }
            let bounds = orb_bounds(orb.visual.position);
            bounds.max.cmpgt(query.min).all() && bounds.min.cmplt(query.max).all()
        })
        .min_by_key(|(index, orb)| {
            (
                crate::items::item_section_order(orb.visual.position),
                orb.section_sequence,
                *index,
            )
        })
        .map(|(index, _)| index);
    if let Some(index) = first {
        entities[index].count = entities[index].count.wrapping_add(1);
        entities[index].lifetime = 0;
        true
    } else {
        false
    }
}

fn orb_cell(position: DVec3) -> (i64, i64, i64) {
    (
        position.x.floor() as i64,
        position.y.floor() as i64,
        position.z.floor() as i64,
    )
}

fn orb_candidates(cells: &HashMap<(i64, i64, i64), Vec<usize>>, position: DVec3) -> Vec<usize> {
    let (x, y, z) = orb_cell(position);
    let mut result = Vec::new();
    for dx in -1_i64..=1 {
        for dy in -1_i64..=1 {
            for dz in -1_i64..=1 {
                let key = (
                    x.saturating_add(dx),
                    y.saturating_add(dy),
                    z.saturating_add(dz),
                );
                if let Some(entries) = cells.get(&key) {
                    result.extend_from_slice(entries);
                }
            }
        }
    }
    result
}

fn merge_nearby(
    entities: &mut [Orb],
    removed: &mut [bool],
    active: &Option<BTreeSet<native_world::loaded::ChunkPos>>,
    index: usize,
    candidates: Option<&[usize]>,
) {
    let bounds = orb_bounds(entities[index].visual.position);
    let expanded = BoundingBox {
        min: bounds.min - DVec3::splat(0.5),
        max: bounds.max + DVec3::splat(0.5),
    };
    let id = entities[index].id;
    let value = entities[index].visual.value;
    for candidate in 0..candidates.map_or(entities.len(), <[usize]>::len) {
        let other = candidates.map_or(candidate, |entries| entries[candidate]);
        if other == index
            || removed[other]
            || !Orbs::active(active, entities[other].visual.position)
            || entities[other].id.wrapping_sub(id) % 40 != 0
            || entities[other].visual.value != value
        {
            continue;
        }
        let other_bounds = orb_bounds(entities[other].visual.position);
        if !other_bounds.max.cmpgt(expanded.min).all()
            || !other_bounds.min.cmplt(expanded.max).all()
        {
            continue;
        }
        let count = entities[other].count;
        let age = entities[other].lifetime;
        let unresolved = entities[other].save_unresolved;
        entities[index].count = entities[index].count.wrapping_add(count);
        entities[index].lifetime = entities[index].lifetime.min(age);
        entities[index].save_unresolved |= unresolved;
        removed[other] = true;
    }
}

fn underwater_velocity(velocity: DVec3) -> DVec3 {
    DVec3::new(
        velocity.x * f64::from(0.99_f32),
        (velocity.y + f64::from(0.0005_f32)).min(f64::from(0.06_f32)),
        velocity.z * f64::from(0.99_f32),
    )
}

fn lava_bounce(random: &mut LegacyRandom) -> DVec3 {
    DVec3::new(
        f64::from((random.next_float() - random.next_float()) * 0.2_f32),
        f64::from(0.2_f32),
        f64::from((random.next_float() - random.next_float()) * 0.2_f32),
    )
}

fn attraction(orb: DVec3, player: DVec3, pose: PlayerPose) -> DVec3 {
    let toward = player + DVec3::Y * (f64::from(pose.eye_height()) / 2.0) - orb;
    let distance = toward.length();
    if distance < 1.0e-4 {
        return DVec3::ZERO;
    }
    let strength = 1.0 - distance / 8.0;
    toward / distance * (strength * strength * 0.1)
}

fn orb_bounds(position: DVec3) -> BoundingBox {
    BoundingBox {
        min: position + DVec3::new(-0.25, 0.0, -0.25),
        max: position + DVec3::new(0.25, 0.5, 0.25),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orb_damage_uses_data_pack_bypass_and_entity_immunity_tags() {
        use native_resources::packs::{ResourceId, ResourceProvider};
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, folder: &str) -> anyhow::Result<Vec<ResourceId>> {
                let ids: &[&str] = match folder {
                    "damage_type" => &["minecraft:lava", "minecraft:on_fire"],
                    "tags/damage_type" => {
                        &["minecraft:is_fall", "minecraft:bypasses_invulnerability"]
                    }
                    _ => &[],
                };
                ids.iter().map(|id| ResourceId::parse(id)).collect()
            }
            fn read_data_stack(&mut self, path: &str) -> anyhow::Result<Vec<Vec<u8>>> {
                let bytes: &[u8] = match path {
                    "data/minecraft/tags/damage_type/is_fall.json" => {
                        br#"{"values":["minecraft:on_fire"]}"#
                    }
                    "data/minecraft/tags/damage_type/bypasses_invulnerability.json" => {
                        br#"{"values":["minecraft:lava"]}"#
                    }
                    "data/minecraft/tags/entity_type/fall_damage_immune.json" => {
                        br#"{"values":["minecraft:experience_orb"]}"#
                    }
                    _ => return Ok(Vec::new()),
                };
                Ok(vec![bytes.to_vec()])
            }
        }
        let tags = OxygenEnchantments::load(&mut Data).unwrap();
        let mut state = OrbDamageState {
            invulnerable: true,
            ..Default::default()
        };
        assert_eq!(orb_lava_contact(&mut state, Some(&tags)), Some(false));
        assert_eq!((state.health, state.fire_ticks), (1, 300));
        state.invulnerable = false;
        state.fire_ticks = 20;
        assert_eq!(orb_fire_tick(&mut state, false, Some(&tags)), Some(false));
        assert_eq!((state.health, state.fire_ticks), (1, 19));
    }

    #[test]
    fn ordered_lava_and_water_effects_damage_and_extinguish_the_orb() {
        use native_world::fluid::FluidKind::{Lava, Water};
        for (effects, expected_fire) in [(vec![Lava, Water], 0), (vec![Water, Lava], 300)] {
            let mut orbs = Orbs::default();
            let mut orb = test_orb(1, DVec3::ZERO, 7, 0, 1);
            orb.no_gravity = true;
            orbs.entities.push(orb);
            let mut inventory = Arc::new(PlayerInventory::default());
            let mut random = LegacyRandom::new(0);
            let mut xp = Experience::default();
            let mut score = 0;
            orbs.tick_with_fluid(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Spectator,
                true,
                &mut inventory,
                &mut random,
                &mut xp,
                &mut score,
                |_| &[],
                |_| 0.6,
                |_| false,
                |_, _| (Default::default(), false),
                false,
                |_, _, _| Some(effects.clone()),
            );
            assert_eq!(orbs.entities.len(), 1);
            assert_eq!(
                (
                    orbs.entities[0].damage.health,
                    orbs.entities[0].damage.fire_ticks
                ),
                (1, expected_fire)
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn lava_and_fire_damage_match_java_orb() {
        use serde_json::Value;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = native_resources::packs::ResourceStack::open(&game).unwrap();
        let tags = OxygenEnchantments::load(&mut source).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let file = temp.path().join("OrbLavaOracle.java");
        std::fs::write(&file, include_str!("../tests/java/OrbLavaOracle.java")).unwrap();
        let run = |command: &mut Command| {
            let out = command.output().unwrap();
            assert!(
                out.status.success(),
                "Java orb lava oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&file));
        let lava = temp.path().join("lava.json");
        let fire = temp.path().join("fire.json");
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("OrbLavaOracle")
            .arg(&lava)
            .arg(&fire));
        for (path, is_lava) in [(lava, true), (fire, false)] {
            let oracle: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            for mode in 0..if is_lava { 2 } else { 4 } {
                let mut state = OrbDamageState {
                    invulnerable: mode % 2 == 1,
                    fire_ticks: if is_lava { 0 } else { 101 },
                    ..Default::default()
                };
                for row in oracle[mode].as_array().unwrap() {
                    let removed = if is_lava {
                        orb_lava_contact(&mut state, Some(&tags))
                    } else {
                        orb_fire_tick(&mut state, mode >= 2, Some(&tags))
                    }
                    .unwrap();
                    assert_eq!(removed, row["removed"].as_bool().unwrap(), "mode {mode}");
                    assert_eq!(
                        i64::from(state.health),
                        row["health"].as_i64().unwrap(),
                        "mode {mode}"
                    );
                    assert_eq!(
                        i64::from(state.fire_ticks),
                        row["fire"].as_i64().unwrap(),
                        "mode {mode}"
                    );
                }
            }
        }
    }

    fn test_orb(id: i32, position: DVec3, value: i32, age: i32, count: i32) -> Orb {
        Orb {
            id,
            section_sequence: 0,
            visual: ExperienceOrb {
                position,
                previous_position: position,
                value,
                age: 0,
            },
            velocity: DVec3::ZERO,
            on_ground: false,
            ground_without_support: false,
            no_gravity: false,
            lifetime: age,
            count,
            random: LegacyRandom::new(0),
            source: None,
            save_unresolved: false,
            damage: OrbDamageState::default(),
        }
    }

    #[test]
    fn indexed_merge_candidates_match_full_scan_including_boundaries() {
        let mut source = Vec::new();
        for i in 0..800_i32 {
            let position = DVec3::new(
                f64::from((i % 37) - 18) * 0.51,
                f64::from((i % 7) - 3) * 0.5,
                f64::from((i % 31) - 15) * 0.49,
            );
            source.push(test_orb(
                i.wrapping_mul(40),
                position,
                if i % 3 == 0 { 7 } else { 3 },
                i - 400,
                i + 1,
            ));
        }
        let mut cells = HashMap::<(i64, i64, i64), Vec<usize>>::new();
        for (index, orb) in source.iter().enumerate() {
            cells
                .entry(orb_cell(orb.visual.position))
                .or_default()
                .push(index);
        }
        for index in [0, 1, 127, 411, 799] {
            let mut linear = source.clone();
            let mut indexed = source.clone();
            let mut a = vec![false; source.len()];
            let mut b = a.clone();
            let candidates = orb_candidates(&cells, source[index].visual.position);
            merge_nearby(&mut linear, &mut a, &None, index, None);
            merge_nearby(&mut indexed, &mut b, &None, index, Some(&candidates));
            assert_eq!(a, b);
            assert_eq!(linear[index].count, indexed[index].count);
            assert_eq!(linear[index].lifetime, indexed[index].lifetime);
        }
    }

    #[test]
    fn first_tick_merges_matching_ids_before_motion_and_preserves_count() {
        let mut orbs = Orbs::default();
        orbs.entities = vec![
            test_orb(1, DVec3::ZERO, 7, 100, 2),
            test_orb(41, DVec3::new(0.99, 0.0, 0.0), 7, 10, 3),
            test_orb(42, DVec3::new(0.5, 0.0, 0.0), 7, 5, 5),
        ];
        let mut inventory = Arc::new(PlayerInventory::default());
        let mut random = LegacyRandom::new(0);
        let mut xp = Experience::default();
        let mut score = 0;
        orbs.tick(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Survival,
            true,
            &mut inventory,
            &mut random,
            &mut xp,
            &mut score,
            |_| &[],
            |_| 0.6,
        );
        assert_eq!(orbs.entities.len(), 2);
        assert_eq!(
            (
                orbs.entities[0].id,
                orbs.entities[0].count,
                orbs.entities[0].lifetime
            ),
            (1, 5, 11)
        );
        assert_eq!(orbs.entities[1].id, 42);
        assert_eq!(orbs.snapshot().len(), 2);
    }

    #[test]
    fn unknown_swept_effects_and_merged_damage_uncertainty_block_drafts() {
        let mut orbs = Orbs::default();
        let mut first = test_orb(1, DVec3::ZERO, 7, 0, 1);
        first.no_gravity = true;
        orbs.entities.push(first);
        let mut inventory = Arc::new(PlayerInventory::default());
        let mut random = LegacyRandom::new(0);
        let mut xp = Experience::default();
        let mut score = 0;
        orbs.tick_with_fluid(
            IVec3::ZERO,
            DVec3::splat(100.0),
            PlayerPose::Standing,
            GameMode::Survival,
            true,
            &mut inventory,
            &mut random,
            &mut xp,
            &mut score,
            |_| &[],
            |_| 0.6,
            |_| false,
            |_, _| (Default::default(), false),
            false,
            |_, _, _| None,
        );
        assert!(orbs.entities[0].save_unresolved);

        let first = test_orb(1, DVec3::ZERO, 7, 0, 1);
        let mut other = test_orb(41, DVec3::new(0.1, 0.0, 0.0), 7, 0, 1);
        other.save_unresolved = true;
        let mut entities = vec![first, other];
        let mut removed = [false, false];
        merge_nearby(&mut entities, &mut removed, &None, 0, None);
        assert!(removed[1]);
        assert!(entities[0].save_unresolved);
    }

    #[test]
    fn merge_scan_repeats_after_twenty_ticks() {
        let mut orbs = Orbs::default();
        orbs.entities = vec![
            test_orb(1, DVec3::ZERO, 7, 10, 1),
            test_orb(41, DVec3::new(2.0, 0.0, 0.0), 7, 20, 1),
        ];
        for orb in &mut orbs.entities {
            orb.no_gravity = true;
        }
        let mut inventory = Arc::new(PlayerInventory::default());
        let mut random = LegacyRandom::new(0);
        let mut xp = Experience::default();
        let mut score = 0;
        let mut tick = |orbs: &mut Orbs| {
            orbs.tick(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Survival,
                true,
                &mut inventory,
                &mut random,
                &mut xp,
                &mut score,
                |_| &[],
                |_| 0.6,
            )
        };
        tick(&mut orbs);
        assert_eq!(orbs.entities.len(), 2);
        orbs.entities[1].visual.position.x = 0.5;
        for _ in 0..19 {
            tick(&mut orbs);
        }
        assert_eq!(orbs.entities.len(), 2);
        tick(&mut orbs);
        assert_eq!(orbs.entities.len(), 1);
        assert_eq!(orbs.entities[0].count, 2);
    }

    #[test]
    fn award_selects_entity_section_then_insertion_order() {
        let mut random = LegacyRandom::new(12);
        let mut preview = random.clone();
        let id = preview.next_int_bound(40).unwrap();
        let mut orbs = vec![
            test_orb(id + 40, DVec3::new(16.1, 4.0, 0.0), 7, 100, 1),
            test_orb(id + 80, DVec3::new(15.9, 4.0, 0.0), 7, 200, 3),
        ];
        assert!(try_merge_award(
            &mut orbs,
            &None,
            DVec3::new(16.0, 4.0, 0.0),
            7,
            &mut random
        ));
        assert_eq!((orbs[0].count, orbs[0].lifetime), (1, 100));
        assert_eq!((orbs[1].count, orbs[1].lifetime), (4, 0));
        assert_eq!(random.next_int(), preview.next_int());
    }

    #[test]
    fn death_award_merges_existing_orb_before_constructing_new_one() {
        let mut orbs = Orbs::default();
        let position = DVec3::new(10.0, 4.0, -2.0);
        let expected_id = orbs.award_random.clone().next_int_bound(40).unwrap() + 40;
        orbs.entities
            .push(test_orb(expected_id, position, 7, 5999, 4));
        let xp = Experience {
            level: 1,
            ..Experience::default()
        };
        orbs.drop_player_experience(position, xp, false, GameMode::Survival);
        assert_eq!(orbs.entities.len(), 1);
        assert_eq!((orbs.entities[0].count, orbs.entities[0].lifetime), (5, 0));
        assert_eq!(orbs.snapshot().len(), 1);
        assert_eq!(
            orbs.next_entity_id, 1,
            "a merge must not allocate an entity"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn award_merging_and_random_consumption_match_java_orb() {
        use serde_json::{Value, json};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("OrbAwardOracle.java");
        std::fs::write(&source, include_str!("../tests/java/OrbAwardOracle.java")).unwrap();
        let run = |command: &mut Command| {
            let out = command.output().unwrap();
            assert!(
                out.status.success(),
                "Java orb award oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source));
        let mut cases = Vec::new();
        let mut expected = Vec::new();
        for offset in [
            -1.001, -1.0, -0.999, -0.751, -0.75, -0.749, 0.0, 0.499, 0.5, 0.501, 0.749, 0.75, 0.751,
        ] {
            for axis in 0..3 {
                for matching in [false, true] {
                    for seed in [0_i64, 17, 12345] {
                        let p = DVec3::new(100.5, 64.0, -2.5);
                        let mut q = p;
                        q[axis] += offset;
                        let mut rng = LegacyRandom::new(seed);
                        let id = rng.clone().next_int_bound(40).unwrap();
                        let other_value = if matching { 7 } else { 3 };
                        let mut orbs = vec![
                            test_orb(id + 40, q, other_value, 99, i32::MAX),
                            test_orb(id + 80, p, 7, 23, 4),
                        ];
                        let encoded: Vec<_> = orbs
                            .iter()
                            .map(|orb| {
                                json!({"position":orb.visual.position.to_array(), "id":orb.id,
                            "value":orb.visual.value, "age":orb.lifetime, "count":orb.count})
                            })
                            .collect();
                        let merged = try_merge_award(&mut orbs, &None, p, 7, &mut rng);
                        expected.push((
                            merged,
                            rng.next_int(),
                            orbs.iter()
                                .map(|o| (o.count, o.lifetime))
                                .collect::<Vec<_>>(),
                        ));
                        cases.push(json!({"position":p.to_array(), "value":7, "seed":seed, "orbs":encoded}));
                    }
                }
            }
        }
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("OrbAwardOracle")
            .arg(input)
            .arg(&output));
        let actual: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (index, (row, (merged, next, orbs))) in actual.iter().zip(expected).enumerate() {
            assert_eq!(row["merged"].as_bool().unwrap(), merged, "case {index}");
            assert_eq!(
                row["next"].as_i64().unwrap(),
                i64::from(next),
                "case {index}"
            );
            for (i, (count, age)) in orbs.into_iter().enumerate() {
                assert_eq!(
                    row["orbs"][i][0].as_i64().unwrap(),
                    i64::from(count),
                    "count case {index}"
                );
                assert_eq!(
                    row["orbs"][i][1].as_i64().unwrap(),
                    i64::from(age),
                    "age case {index}"
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn merge_conditions_and_result_match_java_orb() {
        use serde_json::{Value, json};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("OrbMergeOracle.java");
        std::fs::write(&source, include_str!("../tests/java/OrbMergeOracle.java")).unwrap();
        let run = |command: &mut Command| {
            let out = command.output().unwrap();
            assert!(
                out.status.success(),
                "Java orb merge oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source));
        let mut cases = Vec::new();
        let mut expected = Vec::new();
        for offset in [-1.001, -1.0, -0.999, 0.0, 0.999, 1.0, 1.001] {
            for axis in 0..3 {
                for delta_id in [-40_i32, -1, 0, 1, 40, 80] {
                    for same_value in [false, true] {
                        let p = DVec3::new(100.5, 64.0, -2.5);
                        let mut q = p;
                        q[axis] += offset;
                        let id = 55;
                        let other_id = id + delta_id;
                        let other_value = if same_value { 7 } else { 3 };
                        let mut orbs = vec![
                            test_orb(id, p, 7, 500, i32::MAX - 5),
                            test_orb(other_id, q, other_value, 17, 20),
                        ];
                        let mut removed = [false, false];
                        merge_nearby(&mut orbs, &mut removed, &None, 0, None);
                        expected.push((removed[1], orbs[0].count, orbs[0].lifetime));
                        cases.push(json!({"position":p.to_array(), "other_position":q.to_array(), "id":id, "other_id":other_id,
                            "value":7, "other_value":other_value, "age":500, "other_age":17, "count":i32::MAX-5, "other_count":20}));
                    }
                }
            }
        }
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("OrbMergeOracle")
            .arg(input)
            .arg(&output));
        let actual: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (index, (row, (merged, count, age))) in actual.iter().zip(expected).enumerate() {
            assert_eq!(row["merged"].as_bool().unwrap(), merged, "case {index}");
            assert_eq!(row["removed"].as_bool().unwrap(), merged, "case {index}");
            assert_eq!(
                row["count"].as_i64().unwrap(),
                i64::from(count),
                "case {index}"
            );
            assert_eq!(row["age"].as_i64().unwrap(), i64::from(age), "case {index}");
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn water_and_lava_impulses_match_java_orb() {
        use serde_json::{Value, json};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("OrbFluidOracle.java");
        std::fs::write(&source, include_str!("../tests/java/OrbFluidOracle.java")).unwrap();
        let run = |command: &mut Command| {
            let out = command.output().unwrap();
            assert!(
                out.status.success(),
                "Java orb fluid oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source));
        let mut input = Vec::new();
        let mut expected = Vec::new();
        for index in 0..256_i64 {
            let velocity = DVec3::new(
                (index as f64 * 0.3).sin(),
                (index as f64 - 128.0) / 1000.0,
                (index as f64 * 0.7).cos(),
            );
            let seed = index * 131 - 10000;
            let mut random = LegacyRandom::new(seed);
            let water = underwater_velocity(velocity);
            let lava = lava_bounce(&mut random);
            expected.push((water, lava, random.next_int()));
            input.push(json!({"velocity":velocity.to_array(), "seed":seed}));
        }
        let input_path = temp.path().join("input.json");
        let output_path = temp.path().join("output.json");
        std::fs::write(&input_path, serde_json::to_vec(&input).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("OrbFluidOracle")
            .arg(input_path)
            .arg(&output_path));
        let actual: Vec<Value> =
            serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (index, (row, (water, lava, next))) in actual.iter().zip(expected).enumerate() {
            for (key, want) in [("water", water), ("lava", lava)] {
                let got = &row[key];
                let got = DVec3::new(
                    got[0].as_f64().unwrap(),
                    got[1].as_f64().unwrap(),
                    got[2].as_f64().unwrap(),
                );
                assert_eq!(got, want, "{key} case {index}");
            }
            assert_eq!(row["next"].as_i64().unwrap(), i64::from(next));
            assert_eq!(row["eye"].as_f64().unwrap(), f64::from(0.5_f32 * 0.85_f32));
        }
    }

    #[test]
    fn saved_lifetime_expires_and_unloaded_orb_does_not_age() {
        use native_persistence::nbt::Tag;
        use native_world::loaded::ChunkPos;
        let mut orbs = Orbs::default();
        let home = ChunkPos { x: 0, z: 0 };
        orbs.set_active_chunks(BTreeSet::from([home]));
        orbs.spawn_saved(crate::saved_orb::SavedOrb {
            source: Arc::new(Tag::Compound(Vec::new())),
            uuid: None,
            position: DVec3::new(1.0, 1.0, 1.0),
            velocity: DVec3::ZERO,
            on_ground: false,
            no_gravity: true,
            age: 5999,
            value: 7,
            count: 2,
            health: 5,
            fire_ticks: 0,
            invulnerable: false,
            invulnerable_time: 0,
        });
        orbs.set_active_chunks(BTreeSet::new());
        let mut inventory = Arc::new(PlayerInventory::default());
        let mut random = LegacyRandom::new(0);
        let mut xp = Experience::default();
        let mut score = 0;
        let mut tick = |orbs: &mut Orbs| {
            orbs.tick(
                IVec3::ZERO,
                DVec3::splat(100.0),
                PlayerPose::Standing,
                GameMode::Survival,
                true,
                &mut inventory,
                &mut random,
                &mut xp,
                &mut score,
                |_| &[],
                |_| 0.6,
            );
        };
        tick(&mut orbs);
        assert_eq!(orbs.entities.len(), 1);
        assert_eq!(orbs.entities[0].lifetime, 5999);
        assert!(orbs.snapshot().is_empty());
        orbs.set_active_chunks(BTreeSet::from([home]));
        assert_eq!(orbs.snapshot().len(), 1);
        tick(&mut orbs);
        assert!(orbs.snapshot().is_empty());
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn attraction_matches_vanilla_orb() {
        use serde_json::{Value, json};
        use std::{ffi::OsString, path::PathBuf, process::Command};
        const JAVA: &str = r#"
import java.nio.file.*;
import java.util.UUID;
import com.google.gson.*;
import com.mojang.authlib.GameProfile;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.GameType;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;

public class OrbAttractionOracle {
  static final class TestLevel extends ClientLevel {
    TestLevel() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public int getNextEntityId() { return 1; }
    @Override public boolean noCollision(AABB box) { return true; }
  }
  static final class TestPlayer extends Player {
    TestPlayer(Level level) { super(level,new GameProfile(UUID.randomUUID(),"Oracle")); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
  }
  static JsonArray vec(Vec3 v) { var a=new JsonArray(); a.add(v.x); a.add(v.y); a.add(v.z); return a; }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
    var level=(TestLevel)((sun.misc.Unsafe)unsafeField.get(null)).allocateInstance(TestLevel.class);
    var follow=ExperienceOrb.class.getDeclaredMethod("followNearbyPlayer"); follow.setAccessible(true);
    var target=ExperienceOrb.class.getDeclaredField("followingPlayer"); target.setAccessible(true);
    var inputs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var results=new JsonArray();
    for (var value : inputs) {
      var c=value.getAsJsonObject(); var p=c.getAsJsonArray("player"); var q=c.getAsJsonArray("orb");
      var player=new TestPlayer(level); player.setPos(p.get(0).getAsDouble(),p.get(1).getAsDouble(),p.get(2).getAsDouble());
      var orb=new ExperienceOrb(level,q.get(0).getAsDouble(),q.get(1).getAsDouble(),q.get(2).getAsDouble(),7);
      orb.setDeltaMovement(Vec3.ZERO); target.set(orb,player); follow.invoke(orb);
      results.add(vec(orb.getDeltaMovement()));
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(results));
  }
}
"#;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
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
        let cp: OsString = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("OrbAttractionOracle.java");
        std::fs::write(&source, JAVA).unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "orb oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        let mut cases = Vec::new();
        let mut expected = Vec::new();
        for i in 0..96 {
            let n = f64::from(i);
            let player = DVec3::new(10.0 + n * 0.125, 70.0, -30.0);
            let orb = player
                + DVec3::new(
                    (n * 0.3).sin() * 4.0,
                    (n * 0.2).cos() * 2.0,
                    (n * 0.17).cos() * 4.0,
                );
            cases.push(json!({"player": player.to_array(), "orb": orb.to_array()}));
            expected.push(attraction(orb, player, PlayerPose::Standing));
        }
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("OrbAttractionOracle")
            .arg(&input)
            .arg(&output));
        let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        for (index, (actual, expected)) in results.iter().zip(expected).enumerate() {
            let array = actual.as_array().unwrap();
            let actual = DVec3::new(
                array[0].as_f64().unwrap(),
                array[1].as_f64().unwrap(),
                array[2].as_f64().unwrap(),
            );
            assert!(
                (actual - expected).abs().max_element() < 1e-14,
                "case {index}: {actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn death_values_pickup_delay_and_shared_snapshots() {
        let mut orbs = Orbs::default();
        let old = orbs.snapshot();
        orbs.drop_player_experience(
            DVec3::new(0.0, 2.0, 0.0),
            Experience {
                level: 30,
                ..Default::default()
            },
            false,
            GameMode::Survival,
        );
        assert!(old.is_empty());
        assert_eq!(
            orbs.snapshot()
                .iter()
                .map(|orb| orb.value)
                .collect::<Vec<_>>(),
            [73, 17, 7, 3]
        );
        let mut xp = Experience::default();
        let mut score = 0;
        let mut inv = Arc::new(PlayerInventory::default());
        let mut random = LegacyRandom::new(0);
        for _ in 0..20 {
            orbs.tick(
                IVec3::ZERO,
                DVec3::new(0.0, 1.0, 0.0),
                PlayerPose::Standing,
                GameMode::Survival,
                true,
                &mut inv,
                &mut random,
                &mut xp,
                &mut score,
                |_| &[],
                |_| 0.6,
            );
        }
        assert!(orbs.snapshot().is_empty());
        assert_eq!(score, 100);
        assert_eq!(xp.total, 100);
    }

    #[test]
    fn keep_inventory_and_spectator_do_not_drop_experience() {
        let mut orbs = Orbs::default();
        let xp = Experience {
            level: 8,
            ..Default::default()
        };
        orbs.drop_player_experience(DVec3::ZERO, xp, true, GameMode::Survival);
        orbs.drop_player_experience(DVec3::ZERO, xp, false, GameMode::Spectator);
        assert!(orbs.snapshot().is_empty());
    }

    #[test]
    fn damaged_enchanted_equipment_does_not_lose_unresolved_repair_xp() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        let mut inventory = Arc::new(PlayerInventory::default());
        let mut random = LegacyRandom::new(0);
        let stack = ItemStack::new(
            "minecraft:diamond_pickaxe".into(),
            1,
            1,
            Tag::Compound(vec![
                ("minecraft:damage".into(), Tag::Int(5)),
                (
                    "minecraft:enchantments".into(),
                    Tag::Compound(vec![("minecraft:mending".into(), Tag::Int(1))]),
                ),
            ]),
        )
        .unwrap();
        Arc::make_mut(&mut inventory).set_slot(PlayerSlot::Storage(0), Some(stack));
        let mut orbs = Orbs::default();
        orbs.drop_player_experience(
            DVec3::ZERO,
            Experience {
                level: 1,
                ..Default::default()
            },
            false,
            GameMode::Survival,
        );
        let mut xp = Experience::default();
        let mut score = 0;
        orbs.tick(
            IVec3::ZERO,
            DVec3::ZERO,
            PlayerPose::Standing,
            GameMode::Survival,
            true,
            &mut inventory,
            &mut random,
            &mut xp,
            &mut score,
            |_| &[],
            |_| 0.6,
        );
        assert_eq!(orbs.snapshot().len(), 1);
        assert_eq!(score, 0);
        Arc::make_mut(&mut inventory).set_slot(PlayerSlot::Storage(0), None);
        orbs.tick(
            IVec3::ZERO,
            DVec3::ZERO,
            PlayerPose::Standing,
            GameMode::Survival,
            true,
            &mut inventory,
            &mut random,
            &mut xp,
            &mut score,
            |_| &[],
            |_| 0.6,
        );
        assert!(orbs.snapshot().is_empty());
        assert_eq!(score, 7);
    }

    #[test]
    fn pickup_repairs_equipment_and_credits_only_the_remainder() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        use native_resources::{
            enchantments::OxygenEnchantments,
            packs::{ResourceId, ResourceProvider},
        };
        struct Source;
        impl ResourceProvider for Source {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, folder: &str) -> anyhow::Result<Vec<ResourceId>> {
                if folder == "enchantment" {
                    Ok(vec![ResourceId::parse("minecraft:mending")?])
                } else {
                    Ok(Vec::new())
                }
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok((path == "data/minecraft/enchantment/mending.json").then(||
                    br#"{"slots":["any"],"effects":{"minecraft:repair_with_xp":[{"effect":{"type":"minecraft:multiply","factor":2}}]}}"#.to_vec()))
            }
        }
        let mut inventory = PlayerInventory::default();
        inventory.set_enchantments(Arc::new(OxygenEnchantments::load(&mut Source).unwrap()));
        let stack = ItemStack::new(
            "minecraft:diamond_pickaxe".into(),
            1,
            1,
            Tag::Compound(vec![
                ("minecraft:damage".into(), Tag::Int(5)),
                (
                    "minecraft:enchantments".into(),
                    Tag::Compound(vec![("minecraft:mending".into(), Tag::Int(1))]),
                ),
            ]),
        )
        .unwrap();
        inventory.set_slot(PlayerSlot::Storage(0), Some(stack));
        let mut inventory = Arc::new(inventory);
        let before = Arc::clone(&inventory);
        let mut orbs = Orbs::default();
        orbs.drop_player_experience(
            DVec3::ZERO,
            Experience {
                level: 1,
                ..Default::default()
            },
            false,
            GameMode::Survival,
        );
        let mut rng = LegacyRandom::new(9);
        let mut xp = Experience::default();
        let mut score = 0;
        orbs.tick(
            IVec3::ZERO,
            DVec3::ZERO,
            PlayerPose::Standing,
            GameMode::Survival,
            true,
            &mut inventory,
            &mut rng,
            &mut xp,
            &mut score,
            |_| &[],
            |_| 0.6,
        );
        assert!(orbs.snapshot().is_empty());
        assert_eq!(xp.total, 5);
        assert_eq!(score, 5);
        assert_eq!(
            inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .components()
                .get("minecraft:damage"),
            Some(&Tag::Int(0))
        );
        assert_eq!(
            before
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .components()
                .get("minecraft:damage"),
            Some(&Tag::Int(5))
        );
    }

    #[test]
    fn unsupported_effect_rolls_back_prior_repair_and_random_draws() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        use native_resources::{
            enchantments::OxygenEnchantments,
            packs::{ResourceId, ResourceProvider},
        };
        struct Source;
        impl ResourceProvider for Source {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, folder: &str) -> anyhow::Result<Vec<ResourceId>> {
                if folder == "enchantment" {
                    Ok(vec![
                        ResourceId::parse("example:normal")?,
                        ResourceId::parse("example:conditional")?,
                    ])
                } else {
                    Ok(Vec::new())
                }
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/example/enchantment/normal.json" => br#"{"slots":["any"],"effects":{"minecraft:repair_with_xp":[{"effect":{"type":"minecraft:multiply","factor":2}}]}}"#.as_slice(),
                    "data/example/enchantment/conditional.json" => br#"{"slots":["any"],"effects":{"minecraft:repair_with_xp":[{"effect":{"type":"minecraft:multiply","factor":2},"requirements":{"condition":"minecraft:random_chance","chance":0.5}}]}}"#.as_slice(),
                    _ => return Ok(None),
                };
                Ok(Some(value.to_vec()))
            }
        }
        let stack = |damage, id: &str| {
            ItemStack::new(
                "minecraft:diamond_pickaxe".into(),
                1,
                1,
                Tag::Compound(vec![
                    ("minecraft:damage".into(), Tag::Int(damage)),
                    (
                        "minecraft:enchantments".into(),
                        Tag::Compound(vec![(id.into(), Tag::Int(1))]),
                    ),
                ]),
            )
            .unwrap()
        };
        let mut inventory = PlayerInventory::default();
        inventory.set_enchantments(Arc::new(OxygenEnchantments::load(&mut Source).unwrap()));
        inventory.set_slot(PlayerSlot::Storage(0), Some(stack(1, "example:normal")));
        inventory.set_slot(PlayerSlot::Offhand, Some(stack(30, "example:conditional")));
        let mut inventory = Arc::new(inventory);
        let old = Arc::clone(&inventory);
        let seed = (0..100_000)
            .find(|seed| LegacyRandom::new(*seed).next_int_bound(2) == Some(0))
            .unwrap();
        let mut random = LegacyRandom::new(seed);
        let mut control = random.clone();
        let mut orbs = Orbs::default();
        orbs.drop_player_experience(
            DVec3::ZERO,
            Experience {
                level: 1,
                ..Default::default()
            },
            false,
            GameMode::Survival,
        );
        let mut xp = Experience::default();
        let mut score = 0;
        orbs.tick(
            IVec3::ZERO,
            DVec3::ZERO,
            PlayerPose::Standing,
            GameMode::Survival,
            true,
            &mut inventory,
            &mut random,
            &mut xp,
            &mut score,
            |_| &[],
            |_| 0.6,
        );
        assert!(Arc::ptr_eq(&old, &inventory));
        assert_eq!(random.next_int(), control.next_int());
        assert_eq!(score, 0);
        assert_eq!(orbs.snapshot().len(), 1);
    }
}
