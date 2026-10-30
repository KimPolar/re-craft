//! Experimental in-memory play session over loaded saved chunks.
//! No edits are written to disk. Unimplemented collision states act as barriers.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, OnceLock},
    time::Duration,
};

#[cfg(test)]
use glam::Vec3;
use glam::{DVec3, IVec3};
use native_persistence::heightmap::{
    BlockHeightFlags, HeightBounds, SpawnHeightmapKind, SpawnHeightmaps,
};
use native_resources::enchantments::OxygenEnchantments;
use native_resources::entity_collision::EntityCollisionRegistry;
use native_resources::heightmap_tags::HeightmapTags;
use native_resources::loot::BlockLootTables;
use native_resources::recipes::CraftingRecipes;
use native_resources::transforms::{
    AxeTransformers, BlockTransformers, State as TransformState, ToolKind,
};
use native_world::{
    Block,
    collision::{
        Aabb, block_outline, block_shape, button_support_face, covers_full_face, unknown_barrier,
    },
    fluid::{self, FluidKind, FluidState},
    fluid_sim,
    loaded::{
        BLOCK_COUNT, BlockPosition, BlockState, BlockStateId, ChunkPos, SavedBlockTick,
        SavedFluidTick, Section, World,
    },
    movement::BoundingBox,
    raycast::{ShapeRayHit, raycast_shapes},
};

use crate::{
    CRAWL_HEIGHT, CROUCH_HEIGHT, DeathCause, DeathInfo, DeathRules, GameMode, GameSession,
    MAX_TICKS_PER_UPDATE, PLAYER_HEIGHT, PlayerCommand, PlayerPose, PlayerState, SessionWorld,
    SoilKind, TICK, WorldSnapshot, face_name,
    falling_block::FallingKind,
    interaction_drops,
    inventory::{InventoryMenu, ItemStack, PlayerInventory},
    inventory_click, inventory_pointer, inventory_quick_move, inventory_throw, mining, placed_soil,
    placement_hand, player_intersects_block, select_block, selected_inventory_block,
    supported_transform_state, tool_hands, update_recipe_unlocks, within_crafting_range,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuildBounds {
    pub min_y: i32,
    pub max_y: i32,
}

/// Data needed to attempt an in-memory respawn at an existing saved bed or
/// anchor. Missing chunks, rules and unsupported blocks remain unavailable.
pub struct SavedRespawnSupport {
    pub config: native_persistence::world::PlayerRespawnConfig,
    pub environment: Arc<native_resources::respawn_environment::RespawnEnvironmentRegistry>,
    pub tags: Arc<native_resources::respawn_tags::RespawnBlockTags>,
    pub border: native_persistence::world::SavedWorldBorder,
    pub max_health: f32,
}

/// Settings needed to search the saved world's shared spawn in this dimension.
pub struct WorldRespawnSupport {
    pub spawn: native_persistence::world::RespawnData,
    pub border: native_persistence::world::SavedWorldBorder,
    pub max_health: f32,
    pub radius: i32,
    /// Java uses the world's default game type, not the player's current mode.
    pub adventure: bool,
    /// Supplied only when the generator's spawn height is known for a ceiling.
    pub ceiling_spawn_height: Option<i32>,
}

struct ActiveWorldRespawnSearch {
    search: crate::world_spawn::ResumableWorldSpawn,
    border_tick: u64,
    waiting_revision: Option<(u64, u64)>,
    result_revision: Option<(u64, u64)>,
}

/// Entity-region data is kept separately from rendered block snapshots. An
/// unreadable entry is distinct from a verified empty entity chunk.
#[derive(Clone)]
pub enum SavedEntityData {
    Empty,
    Present(Arc<native_persistence::world::SavedEntityChunk>),
    Unavailable,
}

struct IncomingChunk {
    pos: ChunkPos,
    world: Option<World>,
    entities: Option<SavedEntityData>,
    heightmaps: Option<SpawnHeightmaps>,
}

pub struct LoadedSession {
    world: Arc<World>,
    shapes: Vec<Option<Vec<Aabb>>>,
    outlines: Vec<Option<Vec<Aabb>>>,
    fluids: Vec<Option<FluidState>>,
    origin: IVec3,
    bounds: BuildBounds,
    player: PlayerState,
    camera: crate::camera::CameraMotion,
    motion: crate::physics::MotionState,
    states: [BlockStateId; 5],
    selected: Block,
    inventory: Arc<PlayerInventory>,
    recipes: Option<Arc<CraftingRecipes>>,
    loot: Option<Arc<BlockLootTables>>,
    tool_transforms: [Option<Arc<BlockTransformers>>; 3],
    soil_ticks: BTreeMap<(i128, i32, i64, u64), (IVec3, SoilKind)>,
    soil_deferred: BTreeMap<(i128, i32, i64, u64), (IVec3, SoilKind)>,
    soil_deferred_revision: Option<u64>,
    soil_pending: BTreeSet<(i32, i32, i32, SoilKind)>,
    farmland_maintainers: Option<Arc<BTreeSet<String>>>,
    vanilla_block_ids: Option<Arc<BTreeSet<String>>>,
    dropped_items: crate::items::ItemEntities,
    experience_orbs: crate::orbs::Orbs,
    block_drops: bool,
    entity_drops: bool,
    game_mode: GameMode,
    game_master_permission: bool,
    vitals: crate::survival::PlayerVitals,
    experience: crate::experience::Experience,
    death: Option<DeathInfo>,
    death_processed: bool,
    respawn_death_safe: bool,
    respawn_support: Option<SavedRespawnSupport>,
    saved_respawn_absent: bool,
    world_respawn_support: Option<WorldRespawnSupport>,
    world_respawn_search: Option<ActiveWorldRespawnSearch>,
    world_respawn_needed: bool,
    system_messages: Arc<[crate::SystemMessage]>,
    /// A request retains the border at the time it was accepted while its
    /// destination is prepared across later ticks.
    respawn_request_tick: Option<u64>,
    respawn_target_ready: bool,
    respawn_availability: Option<(u64, u64, u64, bool)>,
    respawn_revision: u64,
    death_rules: DeathRules,
    score: i32,
    armor: crate::equipment_attributes::ArmorTracker,
    survival_rules: crate::survival::SurvivalRules,
    breathing_effects: crate::survival::BreathingEffects,
    effect_save_tracking: Option<EffectSaveTracking>,
    effect_save_invalid: bool,
    oxygen_bonus: f64,
    oxygen_equipment: Option<(crate::player_data::SavedAttribute, Arc<OxygenEnchantments>)>,
    oxygen_inventory_revision: Option<u64>,
    oxygen_valid: bool,
    bounciness_attribute: crate::player_data::SavedAttribute,
    bounciness_inventory_revision: Option<u64>,
    bounciness: Option<f64>,
    jump_strength_attribute: crate::player_data::SavedAttribute,
    jump_strength_inventory_revision: Option<u64>,
    jump_strength: Option<f64>,
    gravity_attribute: crate::player_data::SavedAttribute,
    gravity_inventory_revision: Option<u64>,
    gravity: Option<f64>,
    no_gravity: bool,
    water_efficiency_attribute: crate::player_data::SavedAttribute,
    water_efficiency_inventory_revision: Option<u64>,
    water_efficiency: Option<f64>,
    mining_attributes: crate::player_data::SavedMiningAttributes,
    mining_attribute_revision: Option<u64>,
    mining_attribute_values: [f64; 3],
    movement_speed_attribute: crate::player_data::SavedAttribute,
    movement_speed_cache: Option<(u64, Option<u8>, Option<u8>, [f32; 2])>,
    player_random: native_core::random::LegacyRandom,
    level_random: native_core::random::LegacyRandom,
    flight_speed: f32,
    walking_speed: f32,
    flight: crate::physics::FlightState,
    fov_effect_scale: f32,
    food_use: crate::survival::FoodUse,
    use_repeat: crate::interaction::UseRepeat,
    mining: mining::Mining,
    first_person: crate::animation::FirstPersonState,
    commands: VecDeque<PlayerCommand>,
    movement: (f32, f32, bool),
    sprint: crate::physics::SprintState,
    sprint_window: u8,
    sneak: bool,
    previous_sneak: bool,
    previous_eye_in_water: bool,
    swimming: bool,
    fall_distance: f64,
    ground_without_support: bool,
    // Absolute coordinates survive streaming origin changes.
    ground_support: Option<IVec3>,
    fall: crate::fall::FallTracker,
    accumulated: Duration,
    tick: u64,
    revision: u64,
    snapshot: Arc<WorldSnapshot>,
    unsupported_states: usize,
    dirty_chunks: BTreeSet<ChunkPos>,
    tick_dirty_chunks: BTreeSet<ChunkPos>,
    tick_chunks_needing_dirty: BTreeSet<ChunkPos>,
    block_edits: BTreeMap<ChunkPos, BTreeMap<(i32, i32, i32), RecordedBlockEdit>>,
    parked_chunks: BTreeMap<ChunkPos, Arc<native_world::loaded::Chunk>>,
    entity_chunks: BTreeMap<ChunkPos, SavedEntityData>,
    item_chunks_seen: BTreeSet<ChunkPos>,
    saved_item_uuids: BTreeMap<[i32; 4], ChunkPos>,
    orb_chunks_seen: BTreeSet<ChunkPos>,
    saved_orb_uuids: BTreeMap<[i32; 4], ChunkPos>,
    falling_chunks_seen: BTreeSet<ChunkPos>,
    saved_falling_uuids: BTreeMap<[i32; 4], ChunkPos>,
    entity_collision_cache: BTreeMap<ChunkPos, OnceLock<Result<(), String>>>,
    entity_collision_registry: Option<Arc<EntityCollisionRegistry>>,
    heightmap_tags: Option<Arc<HeightmapTags>>,
    heightmap_flags: Vec<Option<BlockHeightFlags>>,
    heightmap_air: Option<BlockHeightFlags>,
    spawn_heightmaps: BTreeMap<ChunkPos, SpawnHeightmaps>,
    stream_window: Option<(ChunkPos, u32)>,
    pending_window: Option<(ChunkPos, u32)>,
    respawn_window: Option<(ChunkPos, u32)>,
    pending_respawn_window: Option<Option<(ChunkPos, u32)>>,
    incoming: VecDeque<IncomingChunk>,
    stream_errors: Vec<String>,
    fluid_ticks: BTreeMap<(i128, i32, i64, u64), ScheduledFluid>,
    fluid_pending: BTreeSet<FluidTickKey>,
    fluid_sequence: u64,
    fluid_chunks_seen: BTreeSet<ChunkPos>,
    sand_ticks: BTreeMap<(i128, i32, i64, u64), SandTickKey>,
    sand_deferred: BTreeMap<(i128, i32, i64, u64), SandTickKey>,
    sand_deferred_revision: Option<u64>,
    sand_pending: BTreeSet<SandTickKey>,
    button_ticks: ButtonTickQueue,
    button_deferred: ButtonTickQueue,
    button_deferred_revision: Option<u64>,
    button_pending: BTreeSet<(i32, i32, i32)>,
    block_tick_sequence: u64,
    button_sounds: VecDeque<ButtonSoundEvent>,
    falling_sand: Vec<FallingSand>,
    falling_fire_tag: Option<Arc<BTreeSet<String>>>,
    bounce_suppression_tag: Option<Arc<BTreeSet<String>>>,
    world_clocks:
        BTreeMap<native_persistence::world::DimensionId, native_persistence::world::SavedClock>,
    advance_time: bool,
    fast_lava: bool,
    fast_lava_environment: Option<Arc<native_resources::fluid_environment::FastLavaEnvironment>>,
    water_source_conversion: bool,
    lava_source_conversion: bool,
    block_notifications: VecDeque<(IVec3, BlockStateId, BlockStateId)>,
    notifying_blocks: bool,
}

struct EffectSaveTracking {
    breathing: crate::survival::BreathingEffects,
    fall: crate::survival::FallEffects,
    movement_speed: crate::player_data::SavedAttribute,
    gravity: crate::player_data::SavedAttribute,
    water_efficiency: crate::player_data::SavedAttribute,
    mining_attributes: crate::player_data::SavedMiningAttributes,
    ticks: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FluidTickKey {
    x: i32,
    y: i32,
    z: i32,
    kind: FluidKind,
    source: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SandTickKey {
    x: i32,
    y: i32,
    z: i32,
    kind: FallingKind,
}

type ButtonTickQueue = BTreeMap<(i128, i32, i64, u64), (i32, i32, i32)>;

/// Transient, ordered presentation events emitted by completed game ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonSoundEvent {
    pub position: IVec3,
    pub powered: bool,
    pub tick: u64,
}

#[derive(Clone, Debug)]
struct FallingSand {
    motion: crate::falling_block::AirborneMotion,
    previous: DVec3,
    kind: FallingKind,
    drop_item: bool,
    cancel_drop: bool,
    on_ground: bool,
    source: Option<Arc<native_persistence::nbt::Tag>>,
}

#[derive(Clone, Copy, Debug)]
struct RecordedBlockEdit {
    /// None means the source section had no known block data.
    original: Option<BlockStateId>,
    current: BlockStateId,
}

#[derive(Clone, Copy, Debug)]
struct ScheduledFluid {
    key: FluidTickKey,
}

fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: name.into(),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    }
}

fn fresh_breathing_seed() -> i64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static UNIQUE: AtomicU64 = AtomicU64::new(8_682_522_807_148_012);
    let previous = UNIQUE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.wrapping_mul(1_181_783_497_276_652_981))
        })
        .unwrap_or_default();
    let unique = previous.wrapping_mul(1_181_783_497_276_652_981);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    (unique ^ now) as i64
}

impl LoadedSession {
    /// Restore health and food before play begins. World files remain untouched.
    pub fn set_player_vitals(&mut self, vitals: crate::survival::PlayerVitals) {
        self.vitals = vitals;
        self.death = (vitals.health <= 0.0).then_some(DeathInfo {
            cause: DeathCause::Unknown,
            tick: self.tick,
            score: self.score,
            show_message: self.death_rules.show_death_messages,
            inventory_unresolved: false,
            respawn_pending: false,
            can_respawn: false,
        });
        // A player already dead in the source file may have dropped inventory
        // before that file was saved. Do not replay an unknown prior death.
        self.death_processed = vitals.health <= 0.0;
        self.respawn_death_safe = false;
        self.respawn_availability = None;
        self.respawn_request_tick = None;
        self.respawn_target_ready = false;
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.vitals = vitals;
        snapshot.death = self.death;
    }

    pub fn set_player_score(&mut self, score: i32) {
        self.score = score;
        if let Some(death) = &mut self.death {
            death.score = score;
        }
        Arc::make_mut(&mut self.snapshot).death = self.death;
    }

    /// Restore the independent saved experience fields before play begins.
    pub fn set_player_experience(&mut self, experience: crate::experience::Experience) {
        self.experience = experience;
        Arc::make_mut(&mut self.snapshot).experience = experience;
    }

    pub fn set_death_rules(&mut self, rules: DeathRules) {
        self.death_rules = rules;
        if let Some(death) = &mut self.death {
            death.show_message = rules.show_death_messages;
        }
        Arc::make_mut(&mut self.snapshot).death = self.death;
    }

    pub fn set_saved_respawn_support(
        &mut self,
        support: SavedRespawnSupport,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tick == 0,
            "respawn support must be set before the first tick"
        );
        anyhow::ensure!(
            support.max_health.is_finite() && (1.0..=1024.0).contains(&support.max_health),
            "invalid respawn maximum health"
        );
        self.respawn_support = Some(support);
        self.saved_respawn_absent = false;
        self.respawn_availability = None;
        Ok(())
    }

    /// Distinguish an absent player spawn from an unreadable or remote spawn.
    pub fn set_saved_respawn_absent(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tick == 0,
            "respawn support must be set before the first tick"
        );
        self.respawn_support = None;
        self.saved_respawn_absent = true;
        self.respawn_availability = None;
        Ok(())
    }

    pub fn set_world_respawn_support(
        &mut self,
        support: WorldRespawnSupport,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tick == 0,
            "respawn support must be set before the first tick"
        );
        anyhow::ensure!(
            support.max_health.is_finite() && (1.0..=1024.0).contains(&support.max_health),
            "invalid respawn maximum health"
        );
        anyhow::ensure!(
            support.radius >= 0 && support.spawn.yaw.is_finite() && support.spawn.pitch.is_finite(),
            "invalid world spawn settings"
        );
        anyhow::ensure!(
            format!(
                "{}:{}",
                support.spawn.dimension.namespace, support.spawn.dimension.path
            ) == self.world.dimension,
            "world spawn is in another dimension"
        );
        self.world_respawn_support = Some(support);
        self.world_respawn_search = None;
        self.respawn_availability = None;
        Ok(())
    }

    pub fn set_survival_rules(&mut self, rules: crate::survival::SurvivalRules) {
        self.survival_rules = rules;
    }

    /// Restore the effects and persistent oxygen attribute from the saved player.
    pub fn set_player_breathing(
        &mut self,
        effects: crate::survival::BreathingEffects,
        oxygen_bonus: f64,
    ) -> Result<(), &'static str> {
        if !oxygen_bonus.is_finite() || !(0.0..=1024.0).contains(&oxygen_bonus) {
            return Err("invalid oxygen bonus");
        }
        self.effect_save_invalid |= self.tick != 0;
        self.breathing_effects = effects;
        self.oxygen_bonus = oxygen_bonus;
        self.oxygen_equipment = None;
        self.oxygen_inventory_revision = None;
        self.oxygen_valid = true;
        self.bounciness_inventory_revision = None;
        self.jump_strength_inventory_revision = None;
        self.gravity_inventory_revision = None;
        self.water_efficiency_inventory_revision = None;
        self.mining_attribute_revision = None;
        self.movement_speed_cache = None;
        Ok(())
    }

    /// Resolve the saved attribute together with active data-pack equipment effects.
    pub fn set_saved_breathing(
        &mut self,
        effects: crate::survival::BreathingEffects,
        attribute: crate::player_data::SavedAttribute,
        enchantments: Arc<OxygenEnchantments>,
    ) -> anyhow::Result<()> {
        // Java applies transient equipment attributes after its first base tick.
        // Check the saved equipment now, but keep the persistent value for that tick.
        crate::equipment_attributes::oxygen_bonus(&attribute, &self.inventory, &enchantments)?;
        self.effect_save_invalid |= self.tick != 0;
        self.breathing_effects = effects;
        self.oxygen_bonus = attribute.value(0.0, 1024.0)?;
        self.oxygen_inventory_revision = None;
        Arc::make_mut(&mut self.inventory).set_enchantments(Arc::clone(&enchantments));
        self.armor.set_enchantments(Arc::clone(&enchantments));
        if let Err(error) = self.armor.refresh(&self.inventory) {
            self.stream_errors
                .push(format!("player armor equipment: {error:#}"));
        }
        Arc::make_mut(&mut self.snapshot).inventory = Arc::clone(&self.inventory);
        Arc::make_mut(&mut self.snapshot).armor = self.armor.value();
        self.oxygen_equipment = Some((attribute, enchantments));
        self.oxygen_valid = true;
        self.bounciness_inventory_revision = None;
        self.jump_strength_inventory_revision = None;
        self.gravity_inventory_revision = None;
        self.water_efficiency_inventory_revision = None;
        self.mining_attribute_revision = None;
        self.movement_speed_cache = None;
        Ok(())
    }

    /// Restore persistent armor attributes and combine them with equipment.
    pub fn set_saved_armor(
        &mut self,
        armor: crate::player_data::SavedAttribute,
        toughness: crate::player_data::SavedAttribute,
    ) -> anyhow::Result<()> {
        let enchantments = self
            .oxygen_equipment
            .as_ref()
            .map(|(_, registry)| Arc::clone(registry))
            .unwrap_or_default();
        self.armor
            .configure(armor, toughness, enchantments, &self.inventory)?;
        Arc::make_mut(&mut self.snapshot).armor = self.armor.value();
        Ok(())
    }

    /// Restore persistent bounciness; equipment modifiers are applied at tick time.
    pub fn set_saved_bounciness(
        &mut self,
        attribute: crate::player_data::SavedAttribute,
    ) -> anyhow::Result<()> {
        let initial = attribute.value(0.0, 1.0)?;
        self.bounciness_attribute = attribute;
        self.bounciness = Some(initial);
        self.bounciness_inventory_revision = None;
        Ok(())
    }

    fn refresh_bounciness(&mut self) {
        let revision = self.inventory.revision();
        if self.bounciness_inventory_revision == Some(revision) {
            return;
        }
        self.bounciness_inventory_revision = Some(revision);
        let registry = self
            .oxygen_equipment
            .as_ref()
            .map(|(_, registry)| Arc::clone(registry))
            .unwrap_or_default();
        match crate::equipment_attributes::bounciness(
            &self.bounciness_attribute,
            &self.inventory,
            &registry,
        ) {
            Ok(value) => self.bounciness = Some(value),
            Err(error) => {
                self.bounciness = None;
                self.stream_errors
                    .push(format!("player bounciness: {error:#}"));
            }
        }
    }

    /// Restore persistent jump strength; equipment modifiers are applied at tick time.
    pub fn set_saved_jump_strength(
        &mut self,
        attribute: crate::player_data::SavedAttribute,
    ) -> anyhow::Result<()> {
        self.jump_strength = Some(attribute.value(0.0, 32.0)?);
        self.jump_strength_attribute = attribute;
        self.jump_strength_inventory_revision = None;
        Ok(())
    }

    fn refresh_jump_strength(&mut self) {
        let revision = self.inventory.revision();
        if self.jump_strength_inventory_revision == Some(revision) {
            return;
        }
        self.jump_strength_inventory_revision = Some(revision);
        let registry = self
            .oxygen_equipment
            .as_ref()
            .map(|(_, registry)| Arc::clone(registry))
            .unwrap_or_default();
        match crate::equipment_attributes::jump_strength(
            &self.jump_strength_attribute,
            &self.inventory,
            &registry,
        ) {
            Ok(value) => self.jump_strength = Some(value),
            Err(error) => {
                self.jump_strength = None;
                self.stream_errors
                    .push(format!("player jump strength equipment: {error:#}"));
            }
        }
    }

    pub fn restore_saved_gravity(
        &mut self,
        attribute: crate::player_data::SavedAttribute,
        no_gravity: bool,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tick == 0 && self.accumulated == Duration::ZERO,
            "gravity must be restored before the first tick"
        );
        self.gravity = Some(attribute.value(-1.0, 1.0)?);
        self.gravity_attribute = attribute;
        self.gravity_inventory_revision = None;
        self.no_gravity = no_gravity;
        Ok(())
    }

    fn refresh_gravity(&mut self) {
        let revision = self.inventory.revision();
        if self.gravity_inventory_revision == Some(revision) {
            return;
        }
        self.gravity_inventory_revision = Some(revision);
        let registry = self
            .oxygen_equipment
            .as_ref()
            .map(|(_, registry)| Arc::clone(registry))
            .unwrap_or_default();
        match crate::equipment_attributes::gravity(
            &self.gravity_attribute,
            &self.inventory,
            &registry,
        ) {
            Ok(value) => self.gravity = Some(value),
            Err(error) => {
                self.gravity = None;
                self.stream_errors
                    .push(format!("player gravity equipment: {error:#}"));
            }
        }
    }

    pub fn restore_saved_water_movement_efficiency(
        &mut self,
        attribute: crate::player_data::SavedAttribute,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tick == 0 && self.accumulated == Duration::ZERO,
            "water movement efficiency must be restored before the first tick"
        );
        self.water_efficiency = Some(attribute.value(0.0, 1.0)?);
        self.water_efficiency_attribute = attribute;
        self.water_efficiency_inventory_revision = None;
        Ok(())
    }

    fn refresh_water_movement_efficiency(&mut self) {
        let revision = self.inventory.revision();
        if self.water_efficiency_inventory_revision == Some(revision) {
            return;
        }
        self.water_efficiency_inventory_revision = Some(revision);
        let registry = self
            .oxygen_equipment
            .as_ref()
            .map(|(_, registry)| Arc::clone(registry))
            .unwrap_or_default();
        match crate::equipment_attributes::water_movement_efficiency(
            &self.water_efficiency_attribute,
            &self.inventory,
            &registry,
        ) {
            Ok(value) => self.water_efficiency = Some(value),
            Err(error) => {
                self.water_efficiency = None;
                self.stream_errors.push(format!(
                    "player water movement efficiency equipment: {error:#}"
                ));
            }
        }
    }

    pub fn restore_saved_mining_attributes(
        &mut self,
        attributes: crate::player_data::SavedMiningAttributes,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tick == 0 && self.accumulated == Duration::ZERO,
            "mining attributes must be restored before the first tick"
        );
        self.mining_attribute_values = attributes.values()?;
        self.mining_attributes = attributes;
        self.mining_attribute_revision = None;
        Ok(())
    }

    fn refresh_mining_attributes(&mut self) {
        let revision = self.inventory.revision();
        if self.mining_attribute_revision == Some(revision) {
            return;
        }
        self.mining_attribute_revision = Some(revision);
        let registry = self
            .oxygen_equipment
            .as_ref()
            .map(|(_, registry)| Arc::clone(registry))
            .unwrap_or_default();
        match crate::equipment_attributes::mining_attributes(
            &self.mining_attributes,
            &self.inventory,
            &registry,
        ) {
            Ok(values) => self.mining_attribute_values = values,
            Err(error) => {
                self.mining_attribute_values = [0.0, 1.0, 0.2];
                self.stream_errors
                    .push(format!("player mining equipment: {error:#}"));
            }
        }
    }

    fn mining_context(&self) -> crate::mining::MiningContext {
        crate::mining::MiningContext {
            on_ground: self.player.on_ground,
            eyes_in_water: self.previous_eye_in_water,
            mining_efficiency: self.mining_attribute_values[0],
            block_break_speed: self.mining_attribute_values[1],
            submerged_mining_speed: self.mining_attribute_values[2],
            haste: self
                .fall
                .effects()
                .haste
                .as_ref()
                .map(|effect| effect.amplifier),
            conduit_power: self
                .breathing_effects
                .conduit_power
                .as_ref()
                .map(|effect| effect.amplifier),
            mining_fatigue: self
                .fall
                .effects()
                .mining_fatigue
                .as_ref()
                .map(|effect| effect.amplifier),
        }
    }

    /// Restore the saved attribute, including effect modifiers stored by Java.
    pub fn set_saved_movement_speed(
        &mut self,
        attribute: crate::player_data::SavedAttribute,
    ) -> anyhow::Result<()> {
        attribute.value(0.0, 1024.0)?;
        self.effect_save_invalid |= self.tick != 0;
        self.movement_speed_attribute = attribute;
        self.movement_speed_cache = None;
        Ok(())
    }

    fn refresh_movement_speed(&mut self) {
        let revision = self.inventory.revision();
        let speed = self
            .fall
            .effects()
            .speed
            .as_ref()
            .map(|effect| effect.amplifier);
        let slowness = self
            .fall
            .effects()
            .slowness
            .as_ref()
            .map(|effect| effect.amplifier);
        if self
            .movement_speed_cache
            .as_ref()
            .is_some_and(|cache| (cache.0, cache.1, cache.2) == (revision, speed, slowness))
        {
            return;
        }
        let registry = self
            .oxygen_equipment
            .as_ref()
            .map(|(_, registry)| Arc::clone(registry))
            .unwrap_or_default();
        let result = crate::equipment_attributes::equipped_attribute(
            &self.movement_speed_attribute,
            &self.inventory,
            &registry,
            "movement_speed",
        )
        .and_then(|attribute| {
            Ok([
                attribute.movement_speed(None, None, false)?,
                attribute.movement_speed(None, None, true)?,
            ])
        });
        let values = match result {
            Ok(values) => values,
            Err(error) => {
                self.stream_errors
                    .push(format!("player movement speed equipment: {error:#}"));
                [
                    0.1_f32,
                    (f64::from(0.1_f32) * (1.0 + f64::from(0.3_f32))) as f32,
                ]
            }
        };
        self.movement_speed_cache = Some((revision, speed, slowness, values));
    }

    fn movement_speed(&self, sprinting: bool) -> f32 {
        self.movement_speed_cache
            .as_ref()
            .map_or(0.1, |cache| cache.3[usize::from(sprinting)])
    }

    fn update_movement_effect_modifiers(&mut self, transitions: [bool; 2]) {
        use native_resources::enchantments::{AttributeModifier, AttributeOperation};
        use native_resources::packs::ResourceId;
        for (transition, current, name, base) in [
            (
                transitions[0],
                &self.fall.effects().speed,
                "effect.speed",
                f64::from(0.2_f32),
            ),
            (
                transitions[1],
                &self.fall.effects().slowness,
                "effect.slowness",
                f64::from(-0.15_f32),
            ),
        ] {
            if transition {
                let id = ResourceId::parse(&format!("minecraft:{name}"))
                    .expect("vanilla effect identifier");
                self.movement_speed_attribute.remove_modifier(&id);
                if let Some(effect) = current {
                    self.movement_speed_attribute.apply(AttributeModifier {
                        id,
                        operation: AttributeOperation::AddMultipliedTotal,
                        amount: base * (f64::from(effect.amplifier) + 1.0),
                    });
                }
                self.movement_speed_cache = None;
            }
        }
    }

    /// Restore the persistent fall attributes and active status effects.
    pub fn set_saved_fall(
        &mut self,
        safe: crate::player_data::SavedAttribute,
        multiplier: crate::player_data::SavedAttribute,
        effects: crate::survival::FallEffects,
    ) -> anyhow::Result<()> {
        let registry = self
            .oxygen_equipment
            .as_ref()
            .map(|(_, registry)| Arc::clone(registry))
            .ok_or_else(|| anyhow::anyhow!("active enchantment registry is unavailable"))?;
        self.effect_save_invalid |= self.tick != 0;
        self.fall.configure(safe, multiplier, effects, registry);
        self.movement_speed_cache = None;
        Ok(())
    }

    fn refresh_equipment_oxygen(&mut self) {
        let Some((attribute, enchantments)) = &self.oxygen_equipment else {
            return;
        };
        let revision = self.inventory.revision();
        if self.oxygen_inventory_revision == Some(revision) {
            return;
        }
        self.oxygen_inventory_revision = Some(revision);
        match crate::equipment_attributes::oxygen_bonus(attribute, &self.inventory, enchantments) {
            Ok(value) => {
                self.oxygen_bonus = value;
                self.oxygen_valid = true;
            }
            Err(error) => {
                self.oxygen_valid = false;
                self.stream_errors
                    .push(format!("player oxygen equipment: {error:#}"));
            }
        }
    }

    /// Restore the exact position and movement before the first simulation tick.
    /// The renderer's f32 snapshot is separate from the f64 physics state.
    pub fn restore_saved_motion(
        &mut self,
        position: DVec3,
        saved: crate::player_data::SavedPlayerMotion,
    ) -> Result<(), &'static str> {
        if self.tick != 0 || self.accumulated != Duration::ZERO {
            return Err("saved player motion must be restored before the first tick");
        }
        if !position.is_finite()
            || !position.as_vec3().is_finite()
            || !saved.velocity.is_finite()
            || !saved.fall_distance.is_finite()
            || !((saved.velocity.y * 20.0) as f32).is_finite()
        {
            return Err("invalid saved player motion");
        }
        self.motion.position = position;
        self.ground_support = None;
        self.ground_without_support = false;
        self.motion.velocity = saved.velocity;
        self.motion.on_ground = saved.on_ground;
        self.fall_distance = saved.fall_distance;
        self.player.position = position.as_vec3();
        self.player.vertical_velocity = (saved.velocity.y * 20.0) as f32;
        self.player.on_ground = saved.on_ground;
        self.camera = crate::camera::CameraMotion::new(position, self.player.pose.eye_height());
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.player = self.player;
        snapshot.camera = self.camera;
        Ok(())
    }

    /// Restore the player's saved flight speed before starting the session.
    pub fn restore_saved_flight_speed(&mut self, speed: f32) -> Result<(), &'static str> {
        if self.tick != 0 || self.accumulated != Duration::ZERO {
            return Err("saved flight speed must be restored before the first tick");
        }
        if !speed.is_finite() {
            return Err("invalid saved flight speed");
        }
        self.flight_speed = speed;
        Ok(())
    }

    pub fn restore_saved_walking_speed(&mut self, speed: f32) -> Result<(), &'static str> {
        if self.tick != 0 || self.accumulated != Duration::ZERO {
            return Err("saved walking speed must be restored before the first tick");
        }
        if !speed.is_finite() {
            return Err("invalid saved walking speed");
        }
        self.walking_speed = speed;
        Ok(())
    }

    pub fn restore_saved_flying(&mut self, flying: bool) -> Result<(), &'static str> {
        if self.tick != 0 || self.accumulated != Duration::ZERO {
            return Err("saved flight state must be restored before the first tick");
        }
        self.flight.flying = self.game_mode == GameMode::Spectator
            || (self.game_mode == GameMode::Creative && flying);
        Ok(())
    }

    /// Capture simulated player data at a tick boundary. This draft cannot be
    /// written until changed chunks, entities, ticks and other state are handled.
    pub fn draft_player_state(
        &self,
        original: &native_persistence::nbt::NamedTag,
    ) -> anyhow::Result<crate::player_data::PlayerStateDraft> {
        anyhow::ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        anyhow::ensure!(
            crate::player_data::read_saved_walking_speed(original)? == self.walking_speed,
            "saved walking speed does not match the session"
        );
        let initial_movement = self
            .effect_save_tracking
            .as_ref()
            .map_or(&self.movement_speed_attribute, |tracking| {
                &tracking.movement_speed
            });
        anyhow::ensure!(
            crate::player_data::read_loaded_movement_speed_attribute(original)?
                == *initial_movement,
            "saved movement attribute does not match the initial session state"
        );
        anyhow::ensure!(
            crate::player_data::read_saved_no_gravity(original) == self.no_gravity,
            "saved no-gravity state does not match the session"
        );
        let initial_gravity = self
            .effect_save_tracking
            .as_ref()
            .map_or(&self.gravity_attribute, |tracking| &tracking.gravity);
        anyhow::ensure!(
            crate::player_data::read_saved_gravity_attribute(original)? == *initial_gravity,
            "saved gravity attribute does not match the initial session state"
        );
        let initial_water_efficiency = self
            .effect_save_tracking
            .as_ref()
            .map_or(&self.water_efficiency_attribute, |tracking| {
                &tracking.water_efficiency
            });
        anyhow::ensure!(
            crate::player_data::read_saved_water_movement_efficiency_attribute(original)?
                == *initial_water_efficiency,
            "saved water movement efficiency does not match the initial session state"
        );
        let initial_mining = self
            .effect_save_tracking
            .as_ref()
            .map_or(&self.mining_attributes, |tracking| {
                &tracking.mining_attributes
            });
        anyhow::ensure!(
            crate::player_data::read_saved_mining_attributes(original)? == *initial_mining,
            "saved mining attributes do not match the initial session state"
        );
        let effect_draft = if self.tick != 0 {
            anyhow::ensure!(
                !self.effect_save_invalid,
                "player effects changed outside the tracked tick sequence"
            );
            let (initial_breathing, initial_fall, effect_ticks) = match &self.effect_save_tracking {
                Some(tracking) => (&tracking.breathing, &tracking.fall, tracking.ticks),
                None => (&self.breathing_effects, self.fall.effects(), 0),
            };
            anyhow::ensure!(
                crate::player_data::read_saved_breathing_effects(original)? == *initial_breathing
                    && crate::player_data::read_saved_fall_effects(original)? == *initial_fall,
                "saved player effects do not match the initial session state"
            );
            let effects = crate::player_data::advance_saved_effects(original, effect_ticks)?;
            let advanced = native_persistence::nbt::NamedTag {
                name: "".into(),
                value: native_persistence::nbt::Tag::Compound(
                    effects
                        .clone()
                        .map(|tag| ("active_effects".into(), tag))
                        .into_iter()
                        .collect(),
                ),
            };
            anyhow::ensure!(
                crate::player_data::read_saved_breathing_effects(&advanced)?
                    == self.breathing_effects
                    && crate::player_data::read_saved_fall_effects(&advanced)?
                        == *self.fall.effects(),
                "saved effect draft does not match the current session state"
            );
            Some(effects)
        } else {
            None
        };
        let mut yaw = self.player.yaw.to_degrees() - 180.0;
        let mut pitch = -self.player.pitch.to_degrees();
        // Preserve the exact source angles when the player's view has not moved;
        // converting degrees through radians can otherwise change their last bits.
        if let Some(native_persistence::nbt::Tag::List { values, .. }) =
            original.value.get("Rotation")
            && let [
                native_persistence::nbt::Tag::Float(source_yaw),
                native_persistence::nbt::Tag::Float(source_pitch),
            ] = values.as_slice()
        {
            if self.player.yaw == (*source_yaw + 180.0).to_radians() {
                yaw = *source_yaw;
            }
            if self.player.pitch
                == (-*source_pitch)
                    .to_radians()
                    .clamp(-crate::MAX_PITCH, crate::MAX_PITCH)
            {
                pitch = *source_pitch;
            }
        }
        let state = crate::player_data::PlayerSaveState {
            position: self.motion.position + self.origin.as_dvec3(),
            motion: crate::player_data::SavedPlayerMotion {
                velocity: self.motion.velocity,
                on_ground: self.motion.on_ground,
                fall_distance: self.fall_distance,
            },
            yaw,
            pitch,
            dimension: self.world.dimension.clone(),
            game_mode: self.game_mode,
            vitals: self.vitals,
            experience: self.experience,
            score: self.score,
        };
        let mut draft = crate::player_data::draft_player_state(original, &state, &self.inventory)?;
        draft.set_flying(original, self.flight.flying)?;
        if let Some(effects) = effect_draft {
            draft.set_effects(effects)?;
        }
        draft.set_movement_effect_modifiers(
            original,
            initial_movement,
            &self.movement_speed_attribute,
        )?;
        Ok(draft)
    }

    /// Construct a block-state-only draft from a dirty resident or parked chunk.
    /// Lighting, heightmaps, entities and scheduled ticks are not rebuilt here,
    /// so the result is not sufficient to write a playable world.
    pub fn draft_changed_chunk(
        &self,
        position: ChunkPos,
        original: &native_persistence::nbt::NamedTag,
    ) -> anyhow::Result<native_persistence::edit::ChunkDraft> {
        use anyhow::Context;
        anyhow::ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        let edits = self
            .block_edits
            .get(&position)
            .context("chunk has no recorded block edits")?;
        let chunk = self
            .world
            .chunks
            .get(&position)
            .or_else(|| self.parked_chunks.get(&position))
            .context("edited chunk is no longer available")?;
        anyhow::ensure!(
            chunk.position == position,
            "edited chunk position does not match its key"
        );
        let state = |id: BlockStateId| -> anyhow::Result<native_persistence::palette::BlockState> {
            let value = self
                .world
                .state(id)
                .context("edited chunk references an unknown block state")?;
            Ok(native_persistence::palette::BlockState {
                name: value.name.as_str().into(),
                properties: value
                    .properties
                    .iter()
                    .map(|(name, value)| (name.as_str().into(), value.as_str().into()))
                    .collect(),
            })
        };
        let mut changes = Vec::with_capacity(edits.len());
        for (&(x, y, z), edit) in edits {
            let original_id = edit
                .original
                .context("edited block had no known source block data")?;
            let section_y =
                i8::try_from(y.div_euclid(16)).context("edited section is out of range")?;
            let index = x.rem_euclid(16) as usize
                + z.rem_euclid(16) as usize * 16
                + y.rem_euclid(16) as usize * 256;
            let current = chunk
                .sections
                .get(&section_y)
                .and_then(|section| section.blocks.as_ref())
                .context("edited block data is no longer available")?[index];
            anyhow::ensure!(
                current == edit.current,
                "edited block changed outside the recorded session history"
            );
            changes.push(native_persistence::edit::BlockChange {
                x,
                y,
                z,
                expected: state(original_id)?,
                replacement: state(edit.current)?,
            });
        }
        native_persistence::edit::draft_chunk_blocks(
            original,
            native_persistence::world::ChunkPos {
                x: position.x,
                z: position.z,
            },
            &changes,
        )
    }

    /// Combine block edits and supported block ticks in memory only.
    /// The result is not yet safe to write as a playable chunk.
    pub fn draft_changed_chunk_with_block_ticks(
        &self,
        position: ChunkPos,
        original: &native_persistence::nbt::NamedTag,
    ) -> anyhow::Result<native_persistence::edit::ChunkDraft> {
        let ticks = self.draft_block_ticks(position)?;
        let mut draft = self.draft_changed_chunk(position, original)?;
        draft.replace_block_ticks(&ticks)?;
        Ok(draft)
    }

    /// Combine changed blocks and the currently supported block and fluid ticks.
    /// Lighting, entities, other tick types and durable writes remain unfinished.
    pub fn draft_changed_chunk_with_simulated_ticks(
        &self,
        position: ChunkPos,
        original: &native_persistence::nbt::NamedTag,
    ) -> anyhow::Result<native_persistence::edit::ChunkDraft> {
        let fluids = self.draft_fluid_ticks(position)?;
        let mut draft = self.draft_changed_chunk_with_block_ticks(position, original)?;
        draft.replace_fluid_ticks(&fluids)?;
        Ok(draft)
    }

    /// Rebuild known heightmaps in a block draft with active data-pack tags and
    /// caller-provided, authoritative air/fluid state semantics. Other chunk
    /// data still requires coordinated validation before any world write.
    pub fn draft_changed_chunk_with_heightmaps(
        &self,
        position: ChunkPos,
        original: &native_persistence::nbt::NamedTag,
        tags: &native_resources::heightmap_tags::HeightmapTags,
        mut air_and_fluid: impl FnMut(
            &native_persistence::palette::BlockState,
        ) -> anyhow::Result<(bool, bool)>,
    ) -> anyhow::Result<native_persistence::edit::ChunkDraft> {
        use anyhow::Context;
        let mut draft = self.draft_changed_chunk(position, original)?;
        draft.rebuild_heightmaps(
            native_persistence::heightmap::HeightBounds {
                min_y: self.bounds.min_y,
                max_y: self.bounds.max_y,
            },
            |state| {
                let name = String::from_utf16(&state.name.0)
                    .context("block state name is not valid UTF-16")?;
                let motion = tags.for_block(&name)?;
                let (is_air, has_fluid) = air_and_fluid(state)?;
                Ok(native_persistence::heightmap::BlockHeightFlags {
                    is_air,
                    has_fluid,
                    blocks_motion: motion.blocks_motion,
                    blocks_motion_no_leaves: motion.blocks_motion_no_leaves,
                })
            },
        )?;
        Ok(draft)
    }

    /// Combine block edits, known heightmaps, and the simulated block/fluid
    /// schedules into one chunk draft. Lighting, block entities, POI, and other
    /// unsupported world effects still require separate
    /// validation before a playable-world write is safe.
    pub fn draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
        &self,
        position: ChunkPos,
        original: &native_persistence::nbt::NamedTag,
        tags: &native_resources::heightmap_tags::HeightmapTags,
        air_and_fluid: impl FnMut(
            &native_persistence::palette::BlockState,
        ) -> anyhow::Result<(bool, bool)>,
    ) -> anyhow::Result<native_persistence::edit::ChunkDraft> {
        anyhow::ensure!(
            self.dirty_chunk_positions().contains(&position),
            "chunk has no recorded block or scheduled-tick changes"
        );
        let block_ticks = self.draft_block_ticks(position)?;
        let fluid_ticks = self.draft_fluid_ticks(position)?;
        let has_block_edits = self
            .block_edits
            .get(&position)
            .is_some_and(|edits| !edits.is_empty());
        let mut draft = if has_block_edits {
            self.draft_changed_chunk_with_heightmaps(position, original, tags, air_and_fluid)?
        } else {
            self.draft_unchanged_chunk(position, original)?
        };
        draft.replace_block_ticks(&block_ticks)?;
        draft.replace_fluid_ticks(&fluid_ticks)?;
        Ok(draft)
    }

    fn draft_unchanged_chunk(
        &self,
        position: ChunkPos,
        original: &native_persistence::nbt::NamedTag,
    ) -> anyhow::Result<native_persistence::edit::ChunkDraft> {
        use anyhow::{Context, ensure};
        ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        let chunk = self
            .world
            .chunks
            .get(&position)
            .or_else(|| self.parked_chunks.get(&position))
            .context("dirty chunk is no longer available")?;
        ensure!(
            chunk.position == position,
            "dirty chunk position does not match its key"
        );
        native_persistence::edit::draft_chunk_blocks(
            original,
            native_persistence::world::ChunkPos {
                x: position.x,
                z: position.z,
            },
            &[],
        )
    }

    /// Chunks whose block or currently modeled scheduled-tick data changed.
    /// The set is retained until a coordinated world save acknowledges it.
    pub fn dirty_chunk_positions(&self) -> BTreeSet<ChunkPos> {
        self.dirty_chunks
            .union(&self.tick_dirty_chunks)
            .copied()
            .collect()
    }

    fn mark_tick_chunk_dirty(&mut self, x: i32, z: i32) {
        self.tick_dirty_chunks.insert(ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        });
    }

    /// The first world tick changes the serialized delay of saved schedules.
    /// Track only chunks whose original scheduled ticks have not advanced yet.
    fn mark_advanced_saved_tick_chunks_dirty(&mut self) {
        self.tick_dirty_chunks
            .append(&mut self.tick_chunks_needing_dirty);
    }

    fn has_unsaved_chunk_changes(&self, position: ChunkPos) -> bool {
        self.dirty_chunks.contains(&position)
            || self.tick_dirty_chunks.contains(&position)
            || self.tick_chunks_needing_dirty.contains(&position)
    }

    // Activated entity data is the source for drafts even when its terrain is
    // no longer resident. Empty chunks only need retaining when occupied.
    fn retain_entity_chunk(&self, pos: ChunkPos) -> bool {
        let occupied = || {
            self.dropped_items.has_in_chunk(pos)
                || self.experience_orbs.has_in_chunk(pos)
                || self.falling_sand.iter().any(|entity| {
                    let at = entity.motion.position.floor().as_ivec3();
                    at.x.div_euclid(16) == pos.x && at.z.div_euclid(16) == pos.z
                })
        };
        match self.entity_chunks.get(&pos) {
            Some(SavedEntityData::Present(chunk)) => {
                (self.item_chunks_seen.contains(&pos) && !chunk.entities().is_empty()) || occupied()
            }
            Some(SavedEntityData::Empty) => occupied(),
            _ => false,
        }
    }

    pub fn set_game_mode(&mut self, mode: GameMode) {
        self.mining.release();
        self.game_mode = mode;
        if mode == GameMode::Spectator {
            self.flight.flying = true;
        } else if mode != GameMode::Creative {
            self.flight.flying = false;
        }
        Arc::make_mut(&mut self.snapshot).game_mode = mode;
    }

    /// Visual setting applied when the camera next advances.
    pub fn set_fov_effect_scale(&mut self, scale: f32) {
        if scale.is_finite() {
            self.fov_effect_scale = scale.clamp(0.0, 1.0);
        }
    }

    pub fn set_sprint_window(&mut self, ticks: u8) {
        self.sprint_window = ticks.min(10);
    }

    pub fn set_game_master_permission(&mut self, allowed: bool) {
        self.mining.release();
        self.game_master_permission = allowed;
    }

    pub fn set_falling_fire_tag(&mut self, tag: Arc<BTreeSet<String>>) {
        self.falling_fire_tag = Some(tag);
        self.sand_deferred_revision = None;
    }

    pub fn set_bounce_suppression_tag(&mut self, tag: Arc<BTreeSet<String>>) {
        self.bounce_suppression_tag = Some(tag);
    }

    pub fn set_farmland_maintainers(&mut self, tag: Arc<BTreeSet<String>>) {
        self.farmland_maintainers = Some(tag);
        self.soil_deferred_revision = None;
    }

    pub fn set_vanilla_block_ids(&mut self, ids: Arc<BTreeSet<String>>) {
        self.vanilla_block_ids = Some(ids);
        self.soil_deferred_revision = None;
    }

    pub fn set_crafting_recipes(&mut self, recipes: Arc<CraftingRecipes>) {
        self.recipes = Some(recipes);
    }
    pub fn crafting_recipes(&self) -> Option<Arc<CraftingRecipes>> {
        self.recipes.clone()
    }
    pub fn set_block_loot(&mut self, loot: Arc<BlockLootTables>) {
        self.loot = Some(loot);
    }
    pub fn set_axe_transformers(&mut self, transforms: Arc<AxeTransformers>) {
        self.set_tool_transformers(ToolKind::Axe, transforms);
    }
    pub fn set_tool_transformers(&mut self, kind: ToolKind, transforms: Arc<BlockTransformers>) {
        self.tool_transforms[kind.index()] = Some(transforms);
    }
    pub fn set_block_drops(&mut self, enabled: bool) {
        self.block_drops = enabled;
    }

    pub fn set_entity_drops(&mut self, enabled: bool) {
        self.entity_drops = enabled;
    }
    /// Replace the temporary demonstration inventory with data from the saved player.
    pub fn set_player_inventory(&mut self, mut inventory: PlayerInventory) {
        self.mining.release();
        self.first_person.invalidate_inventory();
        self.oxygen_inventory_revision = None;
        self.bounciness_inventory_revision = None;
        self.jump_strength_inventory_revision = None;
        self.gravity_inventory_revision = None;
        self.water_efficiency_inventory_revision = None;
        self.mining_attribute_revision = None;
        self.movement_speed_cache = None;
        self.armor.invalidate();
        if let Some((_, enchantments)) = &self.oxygen_equipment {
            inventory.set_enchantments(Arc::clone(enchantments));
        }
        self.selected = selected_inventory_block(&inventory);
        self.inventory = Arc::new(inventory);
        if let Err(error) = self.armor.refresh(&self.inventory)
            && self.oxygen_equipment.is_some()
        {
            self.stream_errors
                .push(format!("player armor equipment: {error:#}"));
        }
        self.snapshot = Arc::new(WorldSnapshot {
            world: SessionWorld::Loaded(Arc::clone(&self.world)),
            player: self.player,
            camera: self.camera,
            game_mode: self.game_mode,
            vitals: self.vitals,
            experience: self.experience,
            death: self.death,
            armor: self.armor.value(),
            eyes_in_water: self.previous_eye_in_water,
            sprinting: self.sprint.active,
            selected_block: self.selected,
            inventory: Arc::clone(&self.inventory),
            first_person: self.first_person.clone(),
            breaking: None,
            dropped_items: self.dropped_items.snapshot(),
            falling_blocks: self.falling_sand_snapshot(),
            experience_orbs: self.experience_orbs.snapshot(),
            system_messages: Arc::clone(&self.system_messages),
            tick: self.tick,
            origin: self.origin,
            world_revision: self.revision,
        });
    }

    pub fn new(
        mut world: World,
        origin: IVec3,
        bounds: BuildBounds,
        player: PlayerState,
    ) -> Result<Self, &'static str> {
        if bounds.min_y >= bounds.max_y
            || !player.position.is_finite()
            || !player.yaw.is_finite()
            || !player.pitch.is_finite()
            || !player.vertical_velocity.is_finite()
        {
            return Err("invalid world bounds or player position");
        }
        let states = [
            world.intern_state(state("minecraft:air", &[])),
            world.intern_state(state("minecraft:grass_block", &[("snowy", "false")])),
            world.intern_state(state("minecraft:dirt", &[])),
            world.intern_state(state("minecraft:stone", &[])),
            world.intern_state(state("minecraft:stone_bricks", &[])),
        ];
        let shapes: Vec<_> = world
            .states()
            .map(|(_, state)| block_shape(state))
            .collect();
        let outlines: Vec<_> = world
            .states()
            .map(|(_, state)| block_outline(state))
            .collect();
        let fluids: Vec<_> = world
            .states()
            .map(|(_, state)| FluidState::from_block(state).ok().flatten())
            .collect();
        let unsupported_states = shapes.iter().filter(|shape| shape.is_none()).count();
        let saved_ticks = world.saved_fluid_ticks.clone();
        let saved_sand_ticks = world.saved_block_ticks.clone();
        let fluid_chunks_seen = world.chunks.keys().copied().collect();
        let world = Arc::new(world);
        let inventory = Arc::new(PlayerInventory::demo());
        let camera =
            crate::camera::CameraMotion::new(player.position.as_dvec3(), player.pose.eye_height());
        let snapshot = Arc::new(WorldSnapshot {
            world: SessionWorld::Loaded(Arc::clone(&world)),
            player,
            camera,
            game_mode: GameMode::default(),
            vitals: crate::survival::PlayerVitals::default(),
            experience: crate::experience::Experience::default(),
            death: None,
            armor: Some(crate::ArmorValues::default()),
            eyes_in_water: false,
            sprinting: false,
            selected_block: Block::Grass,
            inventory: Arc::clone(&inventory),
            first_person: crate::animation::FirstPersonState::default(),
            breaking: None,
            dropped_items: Arc::from([]),
            falling_blocks: Arc::from([]),
            experience_orbs: Arc::from([]),
            system_messages: Arc::from([]),
            tick: 0,
            origin,
            world_revision: 0,
        });
        let fast_lava = world.dimension == "minecraft:the_nether";
        let mut session = Self {
            world,
            shapes,
            outlines,
            fluids,
            origin,
            bounds,
            player,
            camera,
            motion: crate::physics::MotionState {
                position: player.position.as_dvec3(),
                velocity: DVec3::new(0.0, f64::from(player.vertical_velocity) / 20.0, 0.0),
                on_ground: player.on_ground,
                jump_delay: 0,
            },
            states,
            selected: Block::Grass,
            inventory,
            recipes: None,
            loot: None,
            tool_transforms: [None, None, None],
            soil_ticks: BTreeMap::new(),
            soil_deferred: BTreeMap::new(),
            soil_deferred_revision: None,
            soil_pending: BTreeSet::new(),
            farmland_maintainers: None,
            vanilla_block_ids: None,
            dropped_items: crate::items::ItemEntities::default(),
            experience_orbs: crate::orbs::Orbs::default(),
            block_drops: true,
            entity_drops: true,
            game_mode: GameMode::default(),
            game_master_permission: false,
            vitals: crate::survival::PlayerVitals::default(),
            experience: crate::experience::Experience::default(),
            death: None,
            death_processed: false,
            respawn_death_safe: false,
            respawn_support: None,
            saved_respawn_absent: false,
            world_respawn_support: None,
            world_respawn_search: None,
            world_respawn_needed: false,
            system_messages: Arc::from([]),
            respawn_request_tick: None,
            respawn_target_ready: false,
            respawn_availability: None,
            respawn_revision: 0,
            death_rules: DeathRules::default(),
            score: 0,
            armor: crate::equipment_attributes::ArmorTracker::default(),
            survival_rules: crate::survival::SurvivalRules::default(),
            breathing_effects: crate::survival::BreathingEffects::default(),
            effect_save_tracking: None,
            effect_save_invalid: false,
            oxygen_bonus: 0.0,
            oxygen_equipment: None,
            oxygen_inventory_revision: None,
            oxygen_valid: true,
            bounciness_attribute: crate::player_data::SavedAttribute::with_base(0.0),
            bounciness_inventory_revision: None,
            bounciness: Some(0.0),
            jump_strength_attribute: crate::player_data::SavedAttribute::with_base(f64::from(
                0.42_f32,
            )),
            jump_strength_inventory_revision: None,
            jump_strength: Some(f64::from(0.42_f32)),
            gravity_attribute: crate::player_data::SavedAttribute::with_base(0.08),
            gravity_inventory_revision: None,
            gravity: Some(0.08),
            no_gravity: false,
            water_efficiency_attribute: crate::player_data::SavedAttribute::with_base(0.0),
            water_efficiency_inventory_revision: None,
            water_efficiency: Some(0.0),
            mining_attributes: crate::player_data::SavedMiningAttributes::default(),
            mining_attribute_revision: None,
            mining_attribute_values: [0.0, 1.0, 0.2],
            movement_speed_attribute: crate::player_data::SavedAttribute::with_base(f64::from(
                0.1_f32,
            )),
            movement_speed_cache: None,
            player_random: native_core::random::LegacyRandom::new(fresh_breathing_seed()),
            level_random: native_core::random::LegacyRandom::new(fresh_breathing_seed()),
            flight_speed: 0.05,
            walking_speed: 0.1,
            flight: crate::physics::FlightState::default(),
            fov_effect_scale: 1.0,
            food_use: crate::survival::FoodUse::default(),
            use_repeat: crate::interaction::UseRepeat::default(),
            mining: mining::Mining::default(),
            first_person: crate::animation::FirstPersonState::default(),
            commands: VecDeque::new(),
            movement: (0.0, 0.0, false),
            sprint: crate::physics::SprintState::default(),
            sprint_window: 7,
            sneak: false,
            previous_sneak: false,
            previous_eye_in_water: false,
            swimming: false,
            fall_distance: 0.0,
            ground_without_support: false,
            ground_support: None,
            fall: crate::fall::FallTracker::default(),
            accumulated: Duration::ZERO,
            tick: 0,
            revision: 0,
            snapshot,
            unsupported_states,
            dirty_chunks: BTreeSet::new(),
            tick_dirty_chunks: BTreeSet::new(),
            tick_chunks_needing_dirty: BTreeSet::new(),
            block_edits: BTreeMap::new(),
            parked_chunks: BTreeMap::new(),
            entity_chunks: BTreeMap::new(),
            item_chunks_seen: BTreeSet::new(),
            saved_item_uuids: BTreeMap::new(),
            orb_chunks_seen: BTreeSet::new(),
            saved_orb_uuids: BTreeMap::new(),
            falling_chunks_seen: BTreeSet::new(),
            saved_falling_uuids: BTreeMap::new(),
            entity_collision_cache: BTreeMap::new(),
            entity_collision_registry: None,
            heightmap_tags: None,
            heightmap_flags: Vec::new(),
            heightmap_air: None,
            spawn_heightmaps: BTreeMap::new(),
            stream_window: None,
            pending_window: None,
            respawn_window: None,
            pending_respawn_window: None,
            incoming: VecDeque::new(),
            stream_errors: Vec::new(),
            fluid_ticks: BTreeMap::new(),
            fluid_pending: BTreeSet::new(),
            fluid_sequence: 0,
            fluid_chunks_seen,
            sand_ticks: BTreeMap::new(),
            sand_deferred: BTreeMap::new(),
            sand_deferred_revision: None,
            sand_pending: BTreeSet::new(),
            button_ticks: BTreeMap::new(),
            button_deferred: BTreeMap::new(),
            button_deferred_revision: None,
            button_pending: BTreeSet::new(),
            block_tick_sequence: 0,
            button_sounds: VecDeque::new(),
            falling_sand: Vec::new(),
            falling_fire_tag: None,
            bounce_suppression_tag: None,
            world_clocks: BTreeMap::new(),
            advance_time: true,
            fast_lava,
            fast_lava_environment: None,
            water_source_conversion: true,
            lava_source_conversion: false,
            block_notifications: VecDeque::new(),
            notifying_blocks: false,
        };
        for (pos, ticks) in saved_ticks {
            session.enqueue_saved_fluids(pos, &ticks);
        }
        for (_, ticks) in saved_sand_ticks {
            session.enqueue_saved_sand(&ticks);
            session.enqueue_saved_buttons(&ticks);
            session.enqueue_saved_soil(&ticks);
        }
        Ok(session)
    }

    /// Chunk arrival and eviction are applied before commands on the next tick.
    pub fn queue_chunk_window(&mut self, center: ChunkPos, radius: u32) {
        self.pending_window = Some((center, radius));
    }

    /// The streamer can prepare a saved spawn without adding remote chunks to
    /// the visible, ticking window. The requested window is limited to the
    /// same dimension and only exists after a safely processed death.
    pub fn requested_respawn_window(&self) -> Option<(ChunkPos, u32)> {
        let death = self.death?;
        if self.vitals.hardcore || death.inventory_unresolved || !self.respawn_death_safe {
            return None;
        }
        if self.world_respawn_needed {
            let support = self.world_respawn_support.as_ref()?;
            let position = if let Some(search) = &self.world_respawn_search {
                search.search.requested_position()
            } else {
                let (x, z, _) = self.world_spawn_initial_column(support).ok()?;
                IVec3::new(x, support.spawn.position[1], z)
            };
            return Some((
                ChunkPos {
                    x: position.x.div_euclid(16),
                    z: position.z.div_euclid(16),
                },
                1,
            ));
        }
        let support = self.respawn_support.as_ref()?;
        let (namespace, path) = self.world.dimension.split_once(':')?;
        if namespace != support.config.data.dimension.namespace
            || path != support.config.data.dimension.path
        {
            return None;
        }
        let [x, _, z] = support.config.data.position;
        Some((
            ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            },
            1,
        ))
    }

    pub fn queue_respawn_window(&mut self, window: Option<(ChunkPos, u32)>) {
        self.pending_respawn_window = Some(window);
    }

    pub fn queue_loaded_chunk(&mut self, pos: ChunkPos, chunk_world: World) {
        self.queue_loaded_chunk_with_entities(pos, chunk_world, SavedEntityData::Unavailable);
    }

    pub fn queue_loaded_chunk_with_entities(
        &mut self,
        pos: ChunkPos,
        chunk_world: World,
        entities: SavedEntityData,
    ) {
        self.queue_loaded_chunk_with_data(pos, chunk_world, entities, None);
    }

    /// Deliver a retried entity read without reloading the block chunk.
    pub fn queue_entity_chunk(&mut self, pos: ChunkPos, entities: SavedEntityData) {
        self.incoming.push_back(IncomingChunk {
            pos,
            world: None,
            entities: Some(entities),
            heightmaps: None,
        });
    }

    /// Attach precomputed spawn maps on the same tick as their block chunk.
    pub fn queue_loaded_chunk_with_data(
        &mut self,
        pos: ChunkPos,
        chunk_world: World,
        entities: SavedEntityData,
        heightmaps: Option<SpawnHeightmaps>,
    ) {
        self.incoming.push_back(IncomingChunk {
            pos,
            world: Some(chunk_world),
            entities: Some(entities),
            heightmaps,
        });
    }

    /// Install the active data-pack classification before chunks or edits are
    /// tracked. Unknown states stay unavailable instead of being guessed.
    pub fn set_spawn_heightmap_tags(&mut self, tags: Arc<HeightmapTags>) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tick == 0,
            "heightmap tags must be set before the first tick"
        );
        anyhow::ensure!(
            self.spawn_heightmaps.is_empty(),
            "heightmap tags are already in use"
        );
        let air = crate::spawn_heightmaps::air_flags(&tags)?;
        self.heightmap_flags = crate::spawn_heightmaps::classify_states(&self.world, &tags);
        self.heightmap_air = Some(air);
        self.heightmap_tags = Some(tags);
        Ok(())
    }

    pub fn set_initial_spawn_heightmaps(
        &mut self,
        pos: ChunkPos,
        maps: SpawnHeightmaps,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tick == 0,
            "heightmaps must be set before the first tick"
        );
        anyhow::ensure!(
            self.world.chunks.contains_key(&pos),
            "heightmap chunk is not loaded"
        );
        self.validate_spawn_heightmaps(&maps)?;
        self.spawn_heightmaps.insert(pos, maps);
        Ok(())
    }

    /// Prepare an initial chunk using the already-cached state classifications.
    /// Call this from the world-loading worker, before the session starts.
    pub fn prepare_initial_spawn_heightmaps(
        &mut self,
        saved: &native_persistence::world::LoadedChunk,
    ) -> anyhow::Result<()> {
        use anyhow::Context;
        anyhow::ensure!(
            self.tick == 0,
            "heightmaps must be prepared before the first tick"
        );
        let air = self
            .heightmap_air
            .context("heightmap tags are unavailable")?;
        let bounds = HeightBounds {
            min_y: self.bounds.min_y,
            max_y: self.bounds.max_y,
        };
        let maps = crate::spawn_heightmaps::prepare_with_flags(
            saved,
            &self.world,
            bounds,
            &self.heightmap_flags,
            air,
        )?;
        self.set_initial_spawn_heightmaps(
            ChunkPos {
                x: saved.position.x,
                z: saved.position.z,
            },
            maps,
        )
    }

    fn validate_spawn_heightmaps(&self, maps: &SpawnHeightmaps) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.heightmap_tags.is_some(),
            "heightmap tags are unavailable"
        );
        anyhow::ensure!(maps.is_complete(), "spawn heightmaps have not been primed");
        anyhow::ensure!(
            maps.bounds()
                == HeightBounds {
                    min_y: self.bounds.min_y,
                    max_y: self.bounds.max_y
                },
            "heightmap bounds differ from the dimension"
        );
        Ok(())
    }

    /// Read the current Java heightmap values for a loaded or parked column.
    /// This does not assert that the column has safe collision or entity data.
    pub fn spawn_column_heights(
        &self,
        x: i32,
        z: i32,
    ) -> anyhow::Result<crate::world_spawn::SpawnColumnHeights> {
        use anyhow::Context;
        let pos = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        self.respawn_chunk(IVec3::new(x, self.bounds.min_y, z))?;
        let maps = self
            .spawn_heightmaps
            .get(&pos)
            .context("spawn heightmaps are unavailable")?;
        Ok(crate::world_spawn::SpawnColumnHeights {
            motion_blocking: maps
                .highest(SpawnHeightmapKind::MotionBlocking, x, z)
                .context("motion heightmap is unavailable")?,
            world_surface: maps
                .highest(SpawnHeightmapKind::WorldSurface, x, z)
                .context("surface heightmap is unavailable")?,
            ocean_floor: maps
                .highest(SpawnHeightmapKind::OceanFloor, x, z)
                .context("ocean-floor heightmap is unavailable")?,
        })
    }

    /// Find Java's candidate above one loaded column. The caller supplies the
    /// generator spawn height in ceiling dimensions and must separately check
    /// player, liquid and entity collisions before accepting the candidate.
    pub fn spawn_column_candidate(
        &self,
        x: i32,
        z: i32,
        ceiling_spawn_height: Option<i32>,
    ) -> anyhow::Result<Option<IVec3>> {
        let mut view = LoadedColumnSpawnView {
            session: self,
            ceiling_spawn_height,
        };
        crate::world_spawn::column_spawn(&mut view, x, z)
    }

    /// Run the shared-spawn search over already prepared chunks. Missing
    /// chunks and entities requiring live simulation return an error so an
    /// earlier unresolved candidate cannot be skipped.
    pub fn find_loaded_world_spawn(
        &self,
        origin: IVec3,
        candidates: Option<crate::world_spawn::WorldSpawnCandidates>,
        ceiling_spawn_height: Option<i32>,
    ) -> anyhow::Result<IVec3> {
        let mut view = LoadedColumnSpawnView {
            session: self,
            ceiling_spawn_height,
        };
        crate::world_spawn::find_shared_spawn(&mut view, origin, candidates)
    }

    /// Java's shared-spawn block and liquid collision query for a standing
    /// player. Entity collisions must be checked separately.
    pub fn world_spawn_blocks_clear(&self, pos: IVec3) -> anyhow::Result<bool> {
        let center = pos.as_dvec3() + DVec3::new(0.5, 0.0, 0.5);
        let half = f64::from(0.6_f32 / 2.0);
        let bounds = BoundingBox {
            min: center + DVec3::new(-half, 0.0, -half),
            max: center + DVec3::new(half, f64::from(1.8_f32), half),
        };
        spawn_blocks_clear(self, bounds)
    }

    pub fn set_entity_collision_registry(
        &mut self,
        registry: Arc<EntityCollisionRegistry>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tick == 0,
            "entity collision registry must be set before the first tick"
        );
        self.entity_collision_registry = Some(registry);
        for cached in self.entity_collision_cache.values_mut() {
            cached.take();
        }
        Ok(())
    }

    /// Java first checks block/liquid collisions, then entity collisions. A
    /// saved type whose collision depends on live state remains unresolved.
    pub fn world_spawn_clear(&self, pos: IVec3) -> anyhow::Result<bool> {
        use anyhow::Context;
        if !self.world_spawn_blocks_clear(pos)? {
            return Ok(false);
        }
        const MARGIN: f64 = 2.0 + 1.0e-7;
        let center = pos.as_dvec3() + DVec3::new(0.5, 0.0, 0.5);
        let half = f64::from(0.6_f32 / 2.0);
        let chunk_coord = |value: f64| -> anyhow::Result<i32> {
            let floored = value.floor();
            anyhow::ensure!(
                floored >= f64::from(i32::MIN) && floored <= f64::from(i32::MAX),
                "entity query is out of range"
            );
            Ok((floored as i32).div_euclid(16))
        };
        let min_x = chunk_coord(center.x - half - MARGIN)?;
        let max_x = chunk_coord(center.x + half + MARGIN)?;
        let min_z = chunk_coord(center.z - half - MARGIN)?;
        let max_z = chunk_coord(center.z + half + MARGIN)?;
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let chunk_pos = ChunkPos { x, z };
                match self.entity_chunks.get(&chunk_pos) {
                    Some(SavedEntityData::Empty) => {}
                    Some(SavedEntityData::Present(chunk)) if chunk.entities().is_empty() => {}
                    Some(SavedEntityData::Present(chunk)) => {
                        let registry = self
                            .entity_collision_registry
                            .as_ref()
                            .context("entity collision registry is unavailable")?;
                        let cache = self
                            .entity_collision_cache
                            .get(&chunk_pos)
                            .context("entity collision cache is unavailable")?;
                        let result = cache.get_or_init(|| {
                            crate::spawn_entities::require_noncolliding(chunk.entities(), |id| {
                                registry.kind(id)
                            })
                            .map_err(|error| format!("{error:#}"))
                        });
                        if let Err(error) = result {
                            anyhow::bail!("{error}");
                        }
                    }
                    Some(SavedEntityData::Unavailable) | None => {
                        anyhow::bail!("entity chunk {x}, {z} is unavailable")
                    }
                }
            }
        }
        Ok(true)
    }

    pub fn set_initial_entity_chunk(
        &mut self,
        pos: ChunkPos,
        entities: SavedEntityData,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.tick == 0,
            "entity chunks must be set before the first tick"
        );
        anyhow::ensure!(
            self.world.chunks.contains_key(&pos),
            "entity chunk has no loaded block chunk"
        );
        validate_entity_chunk(pos, &entities)?;
        self.entity_chunks.insert(pos, entities);
        self.entity_collision_cache.insert(pos, OnceLock::new());
        Ok(())
    }

    /// Identity used for saved item ownership checks. Unknown identity never
    /// grants pickup of an item restricted to an owner.
    pub fn set_player_uuid(&mut self, uuid: Option<[i32; 4]>) {
        self.dropped_items.set_player_uuid(uuid);
    }

    pub fn set_item_damage_rules(
        &mut self,
        items: Arc<native_resources::item_registry::ItemRegistry>,
        damage_types: Arc<OxygenEnchantments>,
    ) {
        self.experience_orbs
            .set_damage_tags(Arc::clone(&damage_types));
        self.dropped_items.set_damage_rules(items, damage_types);
    }

    pub fn set_orb_damage_tags(&mut self, damage_types: Arc<OxygenEnchantments>) {
        self.experience_orbs.set_damage_tags(damage_types);
    }

    /// Returns unknown when the installed registry or the prototype cannot be resolved.
    pub fn item_damage_resistance(
        &self,
        stack: &ItemStack,
        damage: &str,
    ) -> anyhow::Result<Option<bool>> {
        self.dropped_items.damage_resistance(stack, damage)
    }

    pub fn saved_entity_chunk(&self, pos: ChunkPos) -> Option<&SavedEntityData> {
        self.entity_chunks.get(&pos)
    }

    /// Draft an existing saved item; this does not authorize a world write.
    pub fn draft_saved_item_entity(
        &self,
        uuid: [i32; 4],
    ) -> anyhow::Result<Option<crate::saved_item::ItemDraft>> {
        anyhow::ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        self.dropped_items.draft_saved_item(uuid)
    }

    /// Prepare newly spawned, unticked items; world save coordination is separate.
    pub fn draft_created_item_entities(
        &self,
        pos: ChunkPos,
    ) -> anyhow::Result<Vec<crate::saved_item::CreatedItemDraft>> {
        anyhow::ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        self.dropped_items.draft_created_items_in_chunk(pos)
    }

    /// Draft a still-live restored falling entity by UUID. Chunk migration,
    /// removals, newly spawned entities and coordinated saving remain separate.
    pub fn draft_saved_falling_entity(
        &self,
        uuid: [i32; 4],
    ) -> anyhow::Result<Option<crate::saved_falling_block::FallingBlockDraft>> {
        use anyhow::ensure;
        ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        for entity in &self.falling_sand {
            let Some(source) = &entity.source else {
                continue;
            };
            if crate::saved_item::optional_uuid(source, "UUID")? != Some(uuid) {
                continue;
            }
            let original = native_persistence::nbt::NamedTag {
                name: "".into(),
                value: (**source).clone(),
            };
            let state = crate::saved_falling_block::FallingSaveState {
                position: entity.motion.position,
                velocity: entity.motion.velocity,
                on_ground: entity.on_ground,
                no_gravity: entity.motion.no_gravity,
                time: entity.motion.age,
                drop_item: entity.drop_item,
                cancel_drop: entity.cancel_drop,
            };
            return Ok(Some(crate::saved_falling_block::draft(&original, state)?));
        }
        Ok(None)
    }

    /// Draft a still-live restored orb without writing world files.
    pub fn draft_saved_orb_entity(
        &self,
        uuid: [i32; 4],
    ) -> anyhow::Result<Option<crate::saved_orb::OrbDraft>> {
        anyhow::ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        self.experience_orbs.draft_saved(uuid)
    }

    /// Build an in-memory entity chunk draft only when no falling entity was
    /// created, removed, or transferred across chunk boundaries.
    pub fn draft_existing_falling_entity_chunk(
        &self,
        pos: ChunkPos,
    ) -> anyhow::Result<crate::saved_falling_block::FallingEntityChunkDraft> {
        let Some(SavedEntityData::Present(chunk)) = self.entity_chunks.get(&pos) else {
            anyhow::bail!("saved entity chunk is unavailable");
        };
        self.reject_unsaved_orbs(pos, chunk)?;
        let replacements = self.collect_existing_falling_drafts(pos, chunk)?;
        crate::saved_falling_block::draft_entity_chunk(&chunk.document, pos, &replacements)
    }

    fn collect_existing_falling_drafts(
        &self,
        pos: ChunkPos,
        chunk: &native_persistence::world::SavedEntityChunk,
    ) -> anyhow::Result<Vec<crate::saved_falling_block::FallingBlockDraft>> {
        use anyhow::{Context, ensure};
        ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        let mut replacements = Vec::new();
        for entity in chunk.entities() {
            let Some(saved) = crate::saved_falling_block::read(entity)? else {
                continue;
            };
            let uuid = saved.uuid.context("falling entity has no stable UUID")?;
            let draft = self
                .draft_saved_falling_entity(uuid)?
                .context("falling entity was removed or could not be simulated")?;
            replacements.push(draft);
        }
        let mut seen = std::collections::BTreeSet::new();
        for entity in &self.falling_sand {
            let current = entity.motion.position.floor().as_ivec3();
            if current.x.div_euclid(16) != pos.x || current.z.div_euclid(16) != pos.z {
                continue;
            }
            let source = entity
                .source
                .as_ref()
                .context("new falling entity needs coordinated creation")?;
            let uuid = crate::saved_item::optional_uuid(source, "UUID")?
                .context("falling entity has no stable UUID")?;
            ensure!(seen.insert(uuid), "duplicate live falling entity UUID");
            ensure!(
                self.saved_falling_uuids.get(&uuid) == Some(&pos),
                "falling entity moved from another chunk"
            );
        }
        Ok(replacements)
    }

    fn reject_unsaved_orbs(
        &self,
        pos: ChunkPos,
        chunk: &native_persistence::world::SavedEntityChunk,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.experience_orbs.has_in_chunk(pos),
            "experience-orb changes cannot yet be saved"
        );
        for entity in chunk.entities() {
            anyhow::ensure!(
                crate::saved_orb::read(entity)?.is_none(),
                "saved experience-orb changes cannot yet be saved"
            );
        }
        Ok(())
    }

    /// Prepare the currently supported saved item, orb and falling-block entities
    /// together. The result is not sufficient to commit a playable world.
    pub fn draft_existing_entity_chunk(
        &self,
        pos: ChunkPos,
    ) -> anyhow::Result<crate::saved_falling_block::FallingEntityChunkDraft> {
        use crate::saved_falling_block::EntityReplacement;
        use anyhow::Context;
        let Some(SavedEntityData::Present(chunk)) = self.entity_chunks.get(&pos) else {
            anyhow::bail!("saved entity chunk is unavailable");
        };
        let falling = self.collect_existing_falling_drafts(pos, chunk)?;
        let mut items = Vec::new();
        let mut orbs = Vec::new();
        for entity in chunk.entities() {
            if let Some(saved) = crate::saved_orb::read(entity)? {
                let uuid = saved.uuid.context("experience orb has no stable UUID")?;
                orbs.push(
                    self.draft_saved_orb_entity(uuid)?
                        .context("experience orb was removed or could not be simulated")?,
                );
            }
            let Some(saved) = crate::saved_item::read(entity)? else {
                continue;
            };
            let uuid = saved.uuid.context("item has no stable UUID")?;
            items.push(
                self.draft_saved_item_entity(uuid)?
                    .context("item was removed or could not be simulated")?,
            );
        }
        self.dropped_items
            .validate_item_chunk_ownership(pos, &self.saved_item_uuids)?;
        self.experience_orbs
            .validate_chunk_group(&BTreeSet::from([pos]), &self.saved_orb_uuids)?;
        let replacements: Vec<_> = falling
            .iter()
            .map(EntityReplacement::Falling)
            .chain(items.iter().map(EntityReplacement::Item))
            .chain(orbs.iter().map(EntityReplacement::Orb))
            .collect();
        crate::saved_falling_block::draft_entity_chunk_with_items(
            &chunk.document,
            pos,
            &replacements,
        )
    }

    /// Prepare a group of known entity chunks including transfers into empty
    /// chunks, including supported saved-item merges and removals. This does
    /// not enable a world write: creation and other effects remain prerequisites.
    pub fn draft_existing_entity_chunks(
        &self,
        positions: &std::collections::BTreeSet<ChunkPos>,
    ) -> anyhow::Result<
        std::collections::BTreeMap<ChunkPos, crate::saved_falling_block::FallingEntityChunkDraft>,
    > {
        use crate::saved_falling_block::EntityReplacement;
        use anyhow::{Context, ensure};

        ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        let mut originals = std::collections::BTreeMap::new();
        let mut falling = Vec::new();
        let mut items = Vec::new();
        let mut removed_items = Vec::new();
        let mut orbs = Vec::new();
        for &pos in positions {
            let chunk = match self.entity_chunks.get(&pos) {
                Some(SavedEntityData::Present(chunk)) => chunk,
                Some(SavedEntityData::Empty) => {
                    originals.insert(pos, None);
                    continue;
                }
                None | Some(SavedEntityData::Unavailable) => {
                    anyhow::bail!("saved entity chunk is unavailable")
                }
            };
            originals.insert(pos, Some(chunk.document.as_ref()));
            for entity in chunk.entities() {
                if let Some(saved) = crate::saved_falling_block::read(entity)? {
                    let uuid = saved.uuid.context("falling entity has no stable UUID")?;
                    falling.push(
                        self.draft_saved_falling_entity(uuid)?
                            .context("falling entity was removed or could not be simulated")?,
                    );
                } else if let Some(saved) = crate::saved_item::read(entity)? {
                    let uuid = saved.uuid.context("item has no stable UUID")?;
                    if let Some(item) = self.draft_saved_item_entity(uuid)? {
                        items.push(item);
                    } else {
                        removed_items.push(
                            self.dropped_items
                                .draft_removed_item(uuid)?
                                .context("item was removed or could not be simulated")?,
                        );
                    }
                } else if let Some(saved) = crate::saved_orb::read(entity)? {
                    let uuid = saved.uuid.context("experience orb has no stable UUID")?;
                    orbs.push(
                        self.draft_saved_orb_entity(uuid)?
                            .context("experience orb was removed or could not be simulated")?,
                    );
                }
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for entity in &self.falling_sand {
            let position = entity.motion.position;
            ensure!(
                position.is_finite()
                    && position.x.abs() <= 30_000_512.0
                    && position.z.abs() <= 30_000_512.0,
                "falling entity has an invalid position"
            );
            let destination = ChunkPos {
                x: (position.x.floor() as i32).div_euclid(16),
                z: (position.z.floor() as i32).div_euclid(16),
            };
            let uuid = entity
                .source
                .as_ref()
                .map(|source| crate::saved_item::optional_uuid(source, "UUID"))
                .transpose()?
                .flatten();
            let source = uuid.and_then(|uuid| self.saved_falling_uuids.get(&uuid).copied());
            if !positions.contains(&destination)
                && !source.is_some_and(|pos| positions.contains(&pos))
            {
                continue;
            }
            let uuid = uuid.context("new falling entity needs coordinated creation")?;
            ensure!(seen.insert(uuid), "duplicate live falling entity UUID");
            ensure!(
                source.is_some_and(|pos| positions.contains(&pos)),
                "falling entity source chunk is unavailable"
            );
            ensure!(
                positions.contains(&destination),
                "falling entity destination chunk is unavailable"
            );
        }
        self.dropped_items
            .validate_item_chunk_group(positions, &self.saved_item_uuids)?;
        self.dropped_items
            .validate_removed_item_chunk_group(positions, &self.saved_item_uuids)?;
        self.experience_orbs
            .validate_chunk_group(positions, &self.saved_orb_uuids)?;
        let replacements: Vec<_> = falling
            .iter()
            .map(EntityReplacement::Falling)
            .chain(items.iter().map(EntityReplacement::Item))
            .chain(orbs.iter().map(EntityReplacement::Orb))
            .collect();
        crate::saved_falling_block::draft_entity_chunks_with_removals(
            &originals,
            &replacements,
            &removed_items,
        )
    }

    /// Collect supported falling-block, button, and soil ticks in Java's saved
    /// sub-tick order. This alone does not authorize writing a chunk.
    pub fn draft_block_ticks(&self, pos: ChunkPos) -> anyhow::Result<Vec<SavedBlockTick>> {
        use anyhow::{Context, ensure};
        ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        ensure!(
            self.fluid_chunks_seen.contains(&pos),
            "chunk ticks have not been loaded"
        );
        if let Some(original) = self.world.saved_block_ticks.get(&pos) {
            ensure!(
                original.iter().all(|tick| {
                    FallingKind::from_id(&tick.block).is_some()
                        || SoilKind::from_name(&tick.block).is_some()
                        || matches!(
                            tick.block.as_str(),
                            "minecraft:stone_button" | "stone_button"
                        )
                }),
                "chunk contains block ticks that are not simulated"
            );
        }
        let mut scheduled = Vec::new();
        for (order, tick) in self
            .sand_ticks
            .iter()
            .chain(&self.sand_deferred)
            .filter(|(_, tick)| tick.x.div_euclid(16) == pos.x && tick.z.div_euclid(16) == pos.z)
        {
            let delay = i32::try_from(order.0 - i128::from(self.tick))
                .context("block tick delay cannot be represented in NBT")?;
            scheduled.push((
                order.2,
                order.3,
                SavedBlockTick {
                    block: tick.kind.id().into(),
                    position: [tick.x, tick.y, tick.z],
                    delay,
                    priority: order.1,
                },
            ));
        }
        let mut queued_buttons = BTreeSet::new();
        for (order, &(x, y, z)) in self
            .button_ticks
            .iter()
            .chain(&self.button_deferred)
            .filter(|(_, (x, _, z))| x.div_euclid(16) == pos.x && z.div_euclid(16) == pos.z)
        {
            queued_buttons.insert((x, y, z));
            let delay = i32::try_from(order.0 - i128::from(self.tick))
                .context("button tick delay cannot be represented in NBT")?;
            scheduled.push((
                order.2,
                order.3,
                SavedBlockTick {
                    block: "minecraft:stone_button".into(),
                    position: [x, y, z],
                    delay,
                    priority: order.1,
                },
            ));
        }
        ensure!(
            self.button_pending
                .iter()
                .filter(|(x, _, z)| x.div_euclid(16) == pos.x && z.div_euclid(16) == pos.z)
                .all(|key| queued_buttons.contains(key)),
            "stone-button tick is missing its scheduled order"
        );
        let mut queued_soil = BTreeSet::new();
        for (order, &(absolute, soil)) in
            self.soil_ticks
                .iter()
                .chain(&self.soil_deferred)
                .filter(|(_, (absolute, _))| {
                    absolute.x.div_euclid(16) == pos.x && absolute.z.div_euclid(16) == pos.z
                })
        {
            queued_soil.insert((absolute.x, absolute.y, absolute.z, soil));
            let delay = i32::try_from(order.0 - i128::from(self.tick))
                .context("soil tick delay cannot be represented in NBT")?;
            scheduled.push((
                order.2,
                order.3,
                SavedBlockTick {
                    block: soil.id().into(),
                    position: absolute.to_array(),
                    delay,
                    priority: order.1,
                },
            ));
        }
        ensure!(
            self.soil_pending
                .iter()
                .filter(|(x, _, z, _)| x.div_euclid(16) == pos.x && z.div_euclid(16) == pos.z)
                .all(|key| queued_soil.contains(key)),
            "soil tick is missing its scheduled order"
        );
        // Java packs saved ticks by sub-tick order, independently of target
        // time and priority. The shared sequence breaks new-tick ties.
        scheduled.sort_unstable_by_key(|(subtick, sequence, _)| (*subtick, *sequence));
        Ok(scheduled.into_iter().map(|(_, _, tick)| tick).collect())
    }

    /// Collect only falling-block ticks for callers that need that subset.
    /// Use `draft_block_ticks` when replacing the chunk's full saved list.
    pub fn draft_falling_block_ticks(&self, pos: ChunkPos) -> anyhow::Result<Vec<SavedBlockTick>> {
        Ok(self
            .draft_block_ticks(pos)?
            .into_iter()
            .filter(|tick| FallingKind::from_id(&tick.block).is_some())
            .collect())
    }

    /// Collect pending ordinary water and lava ticks in Java's saved ordering.
    /// The returned data is only one part of a safe world save.
    pub fn draft_fluid_ticks(&self, pos: ChunkPos) -> anyhow::Result<Vec<SavedFluidTick>> {
        use anyhow::{Context, ensure};
        ensure!(
            self.commands.is_empty(),
            "pending player commands have not reached a tick"
        );
        ensure!(
            self.fluid_chunks_seen.contains(&pos),
            "chunk ticks have not been loaded"
        );
        if let Some(original) = self.world.saved_fluid_ticks.get(&pos) {
            ensure!(
                original.iter().all(|tick| matches!(
                    tick.fluid.as_str(),
                    "minecraft:water"
                        | "water"
                        | "minecraft:flowing_water"
                        | "flowing_water"
                        | "minecraft:lava"
                        | "lava"
                        | "minecraft:flowing_lava"
                        | "flowing_lava"
                )),
                "chunk contains fluid ticks that are not simulated"
            );
        }
        let mut scheduled: Vec<_> = self
            .fluid_ticks
            .iter()
            .filter(|(_, scheduled)| {
                scheduled.key.x.div_euclid(16) == pos.x && scheduled.key.z.div_euclid(16) == pos.z
            })
            .collect();
        scheduled.sort_unstable_by_key(|(order, _)| (order.2, order.3));
        scheduled
            .into_iter()
            .map(|(order, scheduled)| {
                let key = scheduled.key;
                let fluid = match (key.kind, key.source) {
                    (FluidKind::Water, true) => "minecraft:water",
                    (FluidKind::Water, false) => "minecraft:flowing_water",
                    (FluidKind::Lava, true) => "minecraft:lava",
                    (FluidKind::Lava, false) => "minecraft:flowing_lava",
                };
                Ok(SavedFluidTick {
                    fluid: fluid.into(),
                    position: [key.x, key.y, key.z],
                    delay: i32::try_from(order.0 - i128::from(self.tick))
                        .context("fluid tick delay cannot be represented in NBT")?,
                    priority: order.1,
                })
            })
            .collect()
    }

    /// Read supported item documents from a resident or parked entity chunk.
    /// `None` means unavailable, while `Some([])` is a verified empty result.
    /// This is read-only and does not yet instantiate or tick the items.
    pub fn saved_items_in_chunk(
        &self,
        pos: ChunkPos,
    ) -> anyhow::Result<Option<Vec<crate::saved_item::SavedItemState>>> {
        let chunk = match self.entity_chunks.get(&pos) {
            Some(SavedEntityData::Empty) => return Ok(Some(Vec::new())),
            Some(SavedEntityData::Present(chunk)) => chunk,
            Some(SavedEntityData::Unavailable) | None => return Ok(None),
        };
        let mut seen = std::collections::BTreeSet::new();
        let mut items = Vec::new();
        for entity in chunk.entities() {
            if let Some(item) = crate::saved_item::read(entity)? {
                if let Some(uuid) = item.uuid {
                    anyhow::ensure!(
                        seen.insert(uuid),
                        "duplicate saved item UUID in entity chunk"
                    );
                }
                items.push(item);
            }
        }
        Ok(Some(items))
    }

    pub fn queue_missing_chunk(&mut self, pos: ChunkPos) {
        self.incoming.push_back(IncomingChunk {
            pos,
            world: None,
            entities: None,
            heightmaps: None,
        });
    }

    pub fn has_chunk_or_pending(&self, pos: ChunkPos) -> bool {
        self.has_resident_or_parked_chunk(pos)
            || self
                .incoming
                .iter()
                .any(|pending| pending.pos == pos && pending.world.is_some())
    }

    /// A committed chunk cannot disappear without a chunk-window change.
    pub fn has_resident_or_parked_chunk(&self, pos: ChunkPos) -> bool {
        self.world.chunks.contains_key(&pos) || self.parked_chunks.contains_key(&pos)
    }

    pub fn player_chunk(&self) -> Option<ChunkPos> {
        let x = i64::from(self.origin.x).checked_add(self.motion.position.x.floor() as i64)?;
        let z = i64::from(self.origin.z).checked_add(self.motion.position.z.floor() as i64)?;
        let x = i32::try_from(x).ok()?;
        let z = i32::try_from(z).ok()?;
        Some(ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        })
    }

    /// Find a stand-up point around a loaded bed. The caller must separately
    /// verify the dimension's bed rule and the player's respawn configuration.
    pub fn bed_stand_up_position(
        &self,
        pos: IVec3,
        yaw: f32,
        tags: &native_resources::respawn_tags::RespawnBlockTags,
        border: crate::respawn::RespawnBorder,
    ) -> anyhow::Result<Option<DVec3>> {
        let mut view = LoadedDismountView::new(self, tags, border)?;
        let state = view.state_at(pos)?;
        if !is_bed(&state.name) {
            return Ok(None);
        }
        use crate::respawn::HorizontalDirection;
        let facing = match state.properties.get("facing").map(String::as_str) {
            Some("north") => HorizontalDirection::North,
            Some("east") => HorizontalDirection::East,
            Some("south") => HorizontalDirection::South,
            Some("west") => HorizontalDirection::West,
            _ => anyhow::bail!("bed facing is missing or invalid"),
        };
        let below = IVec3::new(pos.x, pos.y.wrapping_sub(1), pos.z);
        let bunk = is_bed(&view.state_at(below)?.name);
        crate::respawn::bed_stand_up_position(pos, facing, yaw, bunk, |candidate, avoid| {
            crate::respawn::find_safe_dismount(&mut view, candidate, avoid)
        })
    }

    /// Find a stand-up point around a loaded anchor. The caller must separately
    /// validate the dimension, charge and charge-consumption rules.
    pub fn anchor_stand_up_position(
        &self,
        pos: IVec3,
        tags: &native_resources::respawn_tags::RespawnBlockTags,
        border: crate::respawn::RespawnBorder,
    ) -> anyhow::Result<Option<DVec3>> {
        let mut view = LoadedDismountView::new(self, tags, border)?;
        if view.state_at(pos)?.name != "minecraft:respawn_anchor" {
            return Ok(None);
        }
        crate::respawn::anchor_stand_up_position(pos, |candidate, avoid| {
            crate::respawn::find_safe_dismount(&mut view, candidate, avoid)
        })
    }

    /// Prepare a saved spawn in this dimension without changing any blocks.
    /// The caller must evaluate the complete environment at the spawn position,
    /// including timelines, and supply Java's `isPossibleToRespawnInThis`
    /// decision for forced spawns. The result's anchor change must be applied
    /// together with the eventual player transition.
    pub fn resolve_saved_spawn(
        &self,
        config: &native_persistence::world::PlayerRespawnConfig,
        environment: native_resources::respawn_environment::RespawnEnvironment,
        dark_outside: bool,
        tags: &native_resources::respawn_tags::RespawnBlockTags,
        border: crate::respawn::RespawnBorder,
        possible: &mut impl FnMut(&BlockState) -> anyhow::Result<bool>,
    ) -> anyhow::Result<Option<crate::respawn::SavedSpawnResult>> {
        let dimension = format!(
            "{}:{}",
            config.data.dimension.namespace, config.data.dimension.path
        );
        anyhow::ensure!(
            self.world.dimension == dimension,
            "saved spawn dimension is not loaded"
        );
        let pos = IVec3::from_array(config.data.position);
        let mut view = LoadedSavedSpawnView {
            dismount: LoadedDismountView::new(self, tags, border)?,
            environment,
            dark_outside,
            possible,
        };
        crate::respawn::resolve_saved_spawn(
            &mut view,
            pos,
            config.data.yaw,
            config.data.pitch,
            config.forced,
            true,
        )
    }

    fn respawn_chunk(&self, pos: IVec3) -> anyhow::Result<&native_world::loaded::Chunk> {
        let key = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        let chunk = self
            .world
            .chunks
            .get(&key)
            .or_else(|| self.parked_chunks.get(&key))
            .ok_or_else(|| {
                anyhow::anyhow!("respawn chunk at {}, {} is not loaded", key.x, key.z)
            })?;
        anyhow::ensure!(
            chunk.status.as_deref() == Some("minecraft:full"),
            "respawn chunk is not complete"
        );
        Ok(chunk)
    }

    fn respawn_state_id(&self, pos: IVec3) -> anyhow::Result<BlockStateId> {
        let chunk = self.respawn_chunk(pos)?;
        if pos.y < self.bounds.min_y || pos.y >= self.bounds.max_y {
            return Ok(self.states[0]);
        }
        let section_y = i8::try_from(pos.y.div_euclid(16))?;
        let index = pos.x.rem_euclid(16) as usize
            + pos.z.rem_euclid(16) as usize * 16
            + pos.y.rem_euclid(16) as usize * 256;
        Ok(chunk
            .sections
            .get(&section_y)
            .and_then(|section| section.blocks.as_ref())
            .map_or(self.states[0], |blocks| blocks[index]))
    }

    fn respawn_biome(&self, pos: IVec3) -> anyhow::Result<&str> {
        let chunk = self.respawn_chunk(pos)?;
        let section_y = i8::try_from(pos.y.div_euclid(16))?;
        let index = (pos.x.rem_euclid(16) / 4) as usize
            + (pos.z.rem_euclid(16) / 4) as usize * 4
            + (pos.y.rem_euclid(16) / 4) as usize * 16;
        let id = chunk
            .sections
            .get(&section_y)
            .and_then(|section| section.biomes.as_ref())
            .ok_or_else(|| anyhow::anyhow!("saved respawn biome is not loaded"))?[index];
        self.world
            .biome_by_id(id)
            .ok_or_else(|| anyhow::anyhow!("unknown saved respawn biome"))
    }

    fn prepare_saved_respawn(&self) -> anyhow::Result<Option<crate::respawn::SavedSpawnResult>> {
        use native_resources::respawn_environment::BedCondition;
        let support = self
            .respawn_support
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("saved respawn is unavailable"))?;
        let pos = IVec3::from_array(support.config.data.position);
        let biome = self.respawn_biome(pos)?;
        let environment = support.environment.for_biome(biome)?;
        let state = self
            .world
            .state(self.respawn_state_id(pos)?)
            .ok_or_else(|| anyhow::anyhow!("saved respawn block is not loaded"))?;
        if is_bed(&state.name) {
            let rule = if state.name == "minecraft:straw_bed" {
                environment.straw_bed
            } else {
                environment.bed
            };
            anyhow::ensure!(
                rule.can_set_spawn != BedCondition::WhenDark,
                "time-dependent bed spawn rules are not yet supported"
            );
        }
        let border = crate::respawn::RespawnBorder::from_saved(support.border, self.tick)?;
        self.resolve_saved_spawn(
            &support.config,
            environment,
            false,
            &support.tags,
            border,
            &mut |_| anyhow::bail!("forced respawn block rules are not yet supported"),
        )
    }

    fn known_saved_respawn(&self) -> anyhow::Result<Option<crate::respawn::SavedSpawnResult>> {
        if self.respawn_support.is_some() {
            self.prepare_saved_respawn()
        } else if self.saved_respawn_absent {
            Ok(None)
        } else {
            anyhow::bail!("saved respawn data is unavailable")
        }
    }

    fn world_spawn_border_tick(&self, support: &WorldRespawnSupport) -> u64 {
        let tick = self.respawn_request_tick.unwrap_or(self.tick);
        if support.border.lerp_time > 0 {
            tick.min(support.border.lerp_time as u64)
        } else {
            0
        }
    }

    /// Java substitutes the border center's motion-blocking height when the
    /// saved shared spawn is outside the current border.
    fn world_spawn_initial_column(
        &self,
        support: &WorldRespawnSupport,
    ) -> anyhow::Result<(i32, i32, bool)> {
        let border = crate::respawn::RespawnBorder::from_saved(
            support.border,
            self.respawn_request_tick.unwrap_or(self.tick),
        )?;
        crate::world_spawn::border_adjusted_column(
            IVec3::from_array(support.spawn.position),
            border,
            support.border.center_x,
            support.border.center_z,
        )
    }

    fn advance_world_respawn(
        &mut self,
    ) -> anyhow::Result<Option<crate::respawn::SavedSpawnResult>> {
        let support = self
            .world_respawn_support
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("world spawn is unavailable"))?;
        let border_tick = self.world_spawn_border_tick(support);
        let revision = (self.revision, self.respawn_revision);
        if self.world_respawn_search.as_ref().is_some_and(|active| {
            active
                .result_revision
                .is_some_and(|result| result != revision)
        }) {
            self.world_respawn_search = None;
        }
        if self.world_respawn_search.is_none() {
            let (x, z, adjusted) = self.world_spawn_initial_column(support)?;
            let y = if adjusted {
                self.spawn_column_heights(x, z)?
                    .motion_blocking
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("world spawn height is out of range"))?
            } else {
                support.spawn.position[1]
            };
            let origin = IVec3::new(x, y, z);
            let candidates = if support.adventure {
                None
            } else {
                let border = crate::respawn::RespawnBorder::from_saved(
                    support.border,
                    self.respawn_request_tick.unwrap_or(self.tick),
                )?;
                let x = f64::from(x);
                let z = f64::from(z);
                let distance = (x - border.min_x)
                    .min(border.max_x - x)
                    .min(z - border.min_z)
                    .min(border.max_z - z);
                let first = crate::world_spawn::WorldSpawnCandidates::new(
                    origin,
                    support.radius,
                    distance,
                    0,
                )?;
                let mut random = native_core::random::LegacyRandom::new(fresh_breathing_seed());
                let offset = random
                    .next_int_bound(first.candidate_count())
                    .expect("positive candidate count");
                Some(crate::world_spawn::WorldSpawnCandidates::new(
                    origin,
                    support.radius,
                    distance,
                    offset,
                )?)
            };
            self.world_respawn_search = Some(ActiveWorldRespawnSearch {
                search: crate::world_spawn::ResumableWorldSpawn::new(origin, candidates)?,
                border_tick,
                waiting_revision: None,
                result_revision: None,
            });
        }
        let mut active = self
            .world_respawn_search
            .take()
            .expect("world spawn search was prepared");
        if active.waiting_revision == Some(revision) {
            self.world_respawn_search = Some(active);
            return Ok(None);
        }
        let mut view = LoadedColumnSpawnView {
            session: self,
            ceiling_spawn_height: support.ceiling_spawn_height,
        };
        // Keep individual ticks responsive even when many columns are unsuitable.
        let polled = active.search.poll(&mut view, 8);
        let result = match polled {
            Ok(result) => {
                active.waiting_revision = None;
                if result.is_some() {
                    active.result_revision = Some(revision);
                }
                Ok(result)
            }
            Err(error) => {
                active.waiting_revision = Some(revision);
                Err(error)
            }
        };
        self.world_respawn_search = Some(active);
        Ok(result?.map(|position| crate::respawn::SavedSpawnResult {
            position: position.as_dvec3() + DVec3::new(0.5, 0.0, 0.5),
            yaw: support.spawn.yaw,
            pitch: support.spawn.pitch,
            anchor_charge: None,
        }))
    }

    fn begin_respawn_request(&mut self) {
        if self.respawn_request_tick.is_some() {
            return;
        }
        self.respawn_request_tick = Some(self.tick);
        self.respawn_availability = None;
        if let (Some(support), Some(search)) =
            (&self.world_respawn_support, &self.world_respawn_search)
            && search.border_tick != self.world_spawn_border_tick(support)
        {
            // A prefetched result used an older border. Start the actual
            // request with the current border, then keep it for this search.
            self.world_respawn_search = None;
        }
    }

    fn refresh_respawn_availability(&mut self) {
        let eligible = self
            .death
            .is_some_and(|death| !self.vitals.hardcore && !death.inventory_unresolved)
            && self.respawn_death_safe;
        let border_tick = self.respawn_support.as_ref().map_or(0, |support| {
            if support.border.lerp_time > 0 {
                self.tick.min(support.border.lerp_time as u64)
            } else {
                0
            }
        });
        let allowed = if !eligible {
            self.world_respawn_needed = false;
            self.world_respawn_search = None;
            self.respawn_request_tick = None;
            false
        } else if !self.world_respawn_needed
            && let Some((revision, respawn_revision, tick, cached)) = self.respawn_availability
            && revision == self.revision
            && respawn_revision == self.respawn_revision
            && tick == border_tick
        {
            cached
        } else {
            match self.known_saved_respawn() {
                Ok(Some(_)) => {
                    self.world_respawn_needed = false;
                    self.world_respawn_search = None;
                    self.respawn_availability =
                        Some((self.revision, self.respawn_revision, border_tick, true));
                    true
                }
                Ok(None) => {
                    self.world_respawn_needed = true;
                    self.respawn_availability = None;
                    if self.world_respawn_support.is_none() {
                        self.respawn_request_tick = None;
                    }
                    matches!(self.advance_world_respawn(), Ok(Some(_)))
                }
                Err(_) => {
                    self.world_respawn_needed = false;
                    self.world_respawn_search = None;
                    self.respawn_availability =
                        Some((self.revision, self.respawn_revision, border_tick, false));
                    false
                }
            }
        };
        self.respawn_target_ready = allowed;
        if let Some(death) = &mut self.death {
            death.can_respawn = allowed;
            death.respawn_pending = self.respawn_request_tick.is_some();
        }
    }

    fn respawn_at_saved_position(&mut self) -> anyhow::Result<bool> {
        let (target, max_health, anchor_pos, missing_spawn) = match self.known_saved_respawn()? {
            Some(target) => {
                let support = self
                    .respawn_support
                    .as_ref()
                    .expect("saved respawn has support");
                (
                    target,
                    support.max_health,
                    Some(IVec3::from_array(support.config.data.position)),
                    false,
                )
            }
            None => {
                let Some(support) = &self.world_respawn_support else {
                    return Ok(false);
                };
                let Some(position) = self
                    .world_respawn_search
                    .as_ref()
                    .and_then(|active| active.search.result())
                else {
                    return Ok(false);
                };
                (
                    crate::respawn::SavedSpawnResult {
                        position: position.as_dvec3() + DVec3::new(0.5, 0.0, 0.5),
                        yaw: support.spawn.yaw,
                        pitch: support.spawn.pitch,
                        anchor_charge: None,
                    },
                    support.max_health,
                    None,
                    self.respawn_support.is_some(),
                )
            }
        };
        anyhow::ensure!(
            target.position.is_finite(),
            "respawn position is not finite"
        );
        let x = target.position.x.floor();
        let z = target.position.z.floor();
        anyhow::ensure!(
            x >= f64::from(i32::MIN)
                && x <= f64::from(i32::MAX)
                && z >= f64::from(i32::MIN)
                && z <= f64::from(i32::MAX),
            "respawn position is outside supported coordinates"
        );
        let destination = ChunkPos {
            x: (x as i32).div_euclid(16),
            z: (z as i32).div_euclid(16),
        };
        let destination_origin = IVec3::new(
            destination
                .x
                .checked_mul(16)
                .ok_or_else(|| anyhow::anyhow!("respawn origin is out of range"))?,
            self.origin.y,
            destination
                .z
                .checked_mul(16)
                .ok_or_else(|| anyhow::anyhow!("respawn origin is out of range"))?,
        );
        let position = target.position - destination_origin.as_dvec3();
        anyhow::ensure!(
            position.is_finite() && position.as_vec3().is_finite(),
            "respawn position is out of range"
        );
        let oxygen_bonus = if let Some((attribute, _)) = &self.oxygen_equipment {
            let mut base = attribute.clone();
            base.clear_modifiers();
            base.value(0.0, 1024.0)?
        } else {
            0.0
        };
        let anchor_update = if let Some(charges) = target.anchor_charge {
            let anchor_pos = anchor_pos
                .ok_or_else(|| anyhow::anyhow!("respawn anchor position is unavailable"))?;
            let mut anchor = self
                .world
                .state(self.respawn_state_id(anchor_pos)?)
                .ok_or_else(|| anyhow::anyhow!("respawn anchor is no longer loaded"))?
                .clone();
            anyhow::ensure!(
                anchor.name == "minecraft:respawn_anchor",
                "respawn anchor changed"
            );
            anchor
                .properties
                .insert("charges".into(), charges.to_string());
            Some(anchor)
        } else {
            None
        };
        if self.origin != destination_origin || !self.world.chunks.contains_key(&destination) {
            let radius = self.stream_window.map_or(1, |(_, radius)| radius.max(1));
            self.pending_window = Some((destination, radius));
            self.apply_streaming();
        }
        anyhow::ensure!(
            self.origin == destination_origin && self.world.chunks.contains_key(&destination),
            "cannot activate respawn chunk"
        );
        if let Some(anchor) = anchor_update {
            let anchor_pos = anchor_pos.expect("anchor update has a position");
            let relative = subtract(anchor_pos, self.origin)
                .ok_or_else(|| anyhow::anyhow!("respawn anchor is out of range"))?;
            let state = self.intern_block(anchor);
            anyhow::ensure!(
                self.set(relative, state),
                "cannot update respawn anchor charge"
            );
        }
        let keep = self.death_rules.keep_inventory || self.game_mode == GameMode::Spectator;
        Arc::make_mut(&mut self.inventory).reset_after_death(keep);
        if !keep {
            self.score = 0;
            self.experience = crate::experience::Experience::default();
        }
        self.player = PlayerState {
            position: position.as_vec3(),
            yaw: (target.yaw + 180.0).to_radians(),
            pitch: (-target.pitch)
                .to_radians()
                .clamp(-crate::MAX_PITCH, crate::MAX_PITCH),
            vertical_velocity: 0.0,
            on_ground: false,
            pose: PlayerPose::Standing,
        };
        self.motion = crate::physics::MotionState {
            position,
            velocity: DVec3::ZERO,
            on_ground: false,
            jump_delay: 0,
        };
        self.camera = crate::camera::CameraMotion::new(position, self.player.pose.eye_height());
        self.vitals = crate::survival::PlayerVitals {
            health: max_health,
            max_health,
            hardcore: self.vitals.hardcore,
            ..Default::default()
        };
        self.death = None;
        self.death_processed = false;
        self.respawn_death_safe = false;
        self.respawn_availability = None;
        self.world_respawn_needed = false;
        self.world_respawn_search = None;
        self.respawn_request_tick = None;
        self.respawn_target_ready = false;
        if missing_spawn {
            self.respawn_support = None;
            self.saved_respawn_absent = true;
            let mut messages = self.system_messages.to_vec();
            if messages.len() >= 100 {
                messages.remove(0);
            }
            messages.push(crate::SystemMessage {
                kind: crate::SystemMessageKind::MissingRespawnBlock,
                tick: self.tick.saturating_add(1),
            });
            self.system_messages = messages.into();
        }
        self.movement = (0.0, 0.0, false);
        self.sprint = crate::physics::SprintState::default();
        self.sneak = false;
        self.previous_sneak = false;
        self.previous_eye_in_water = false;
        self.swimming = false;
        self.fall_distance = 0.0;
        self.ground_without_support = false;
        self.ground_support = None;
        self.bounciness_attribute.clear_modifiers();
        self.bounciness_inventory_revision = None;
        self.bounciness = self.bounciness_attribute.value(0.0, 1.0).ok();
        self.jump_strength_attribute.clear_modifiers();
        self.jump_strength_inventory_revision = None;
        self.jump_strength = self.jump_strength_attribute.value(0.0, 32.0).ok();
        self.gravity_attribute.clear_modifiers();
        self.gravity_inventory_revision = None;
        self.gravity = self.gravity_attribute.value(-1.0, 1.0).ok();
        self.no_gravity = false;
        self.water_efficiency_attribute.clear_modifiers();
        self.water_efficiency_inventory_revision = None;
        self.water_efficiency = self.water_efficiency_attribute.value(0.0, 1.0).ok();
        self.mining_attributes.clear_modifiers();
        self.mining_attribute_revision = None;
        self.mining_attribute_values = self.mining_attributes.values().unwrap_or([0.0, 1.0, 0.2]);
        self.movement_speed_attribute.clear_modifiers();
        self.movement_speed_cache = None;
        self.fall.reset_after_death();
        self.breathing_effects = crate::survival::BreathingEffects::default();
        self.effect_save_invalid = true;
        if let Some((attribute, _)) = &mut self.oxygen_equipment {
            attribute.clear_modifiers();
            self.oxygen_bonus = oxygen_bonus;
            self.oxygen_inventory_revision = None;
            self.oxygen_valid = true;
        } else {
            self.oxygen_bonus = 0.0;
            self.oxygen_valid = true;
        }
        self.player_random = native_core::random::LegacyRandom::new(fresh_breathing_seed());
        self.flight = crate::physics::FlightState::default();
        self.walking_speed = 0.1;
        self.flight.flying = self.game_mode == GameMode::Spectator;
        self.experience_orbs.reset_player_pickup_delay();
        self.food_use = crate::survival::FoodUse::default();
        self.use_repeat = crate::interaction::UseRepeat::default();
        self.mining.release();
        self.first_person = crate::animation::FirstPersonState::default();
        self.selected = selected_inventory_block(&self.inventory);
        self.armor.reset_after_death();
        if let Err(error) = self.armor.refresh(&self.inventory) {
            self.stream_errors
                .push(format!("player armor equipment: {error:#}"));
        }
        Ok(true)
    }

    pub fn take_stream_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.stream_errors)
    }

    /// Set the saved world's source-conversion game rules before ticking.
    pub fn set_source_conversion(&mut self, water: bool, lava: bool) {
        self.water_source_conversion = water;
        self.lava_source_conversion = lava;
    }

    /// Set the dimension-wide environment value before the first tick.
    pub fn set_fast_lava(&mut self, fast: bool) {
        self.fast_lava = fast;
        self.fast_lava_environment = None;
    }

    pub fn set_fast_lava_environment(
        &mut self,
        environment: Arc<native_resources::fluid_environment::FastLavaEnvironment>,
    ) -> Result<(), &'static str> {
        let value = environment
            .sample(|id| {
                self.world_clocks
                    .iter()
                    .find(|(clock, _)| clock.namespace == id.namespace && clock.path == id.path)
                    .map(|(_, state)| state.total_ticks)
            })
            .ok_or("timeline clock is unavailable")?;
        self.fast_lava = value;
        self.fast_lava_environment = Some(environment);
        Ok(())
    }

    /// Install the shared clock states before ticking the loaded world.
    pub fn set_world_clocks(
        &mut self,
        clocks: BTreeMap<
            native_persistence::world::DimensionId,
            native_persistence::world::SavedClock,
        >,
        advance_time: bool,
    ) {
        self.world_clocks = clocks;
        self.advance_time = advance_time;
    }

    pub fn world_clock(
        &self,
        id: &native_persistence::world::DimensionId,
    ) -> Option<native_persistence::world::SavedClock> {
        self.world_clocks.get(id).copied()
    }

    /// Snapshot all shared dimension clocks for a coordinated persistence plan.
    pub fn world_clocks(
        &self,
    ) -> BTreeMap<native_persistence::world::DimensionId, native_persistence::world::SavedClock>
    {
        self.world_clocks.clone()
    }

    fn apply_streaming(&mut self) {
        let mut changed = false;
        let mut respawn_changed = false;
        if let Some(window) = self.pending_respawn_window.take()
            && self.respawn_window != window
        {
            self.respawn_window = window;
            respawn_changed = true;
        }
        if let Some(window) = self.pending_window.take() {
            self.stream_window = Some(window);
            if let (Some(x), Some(z)) = (window.0.x.checked_mul(16), window.0.z.checked_mul(16)) {
                let origin = IVec3::new(x, self.origin.y, z);
                if origin != self.origin {
                    let shift = DVec3::new(
                        f64::from(self.origin.x) - f64::from(origin.x),
                        0.0,
                        f64::from(self.origin.z) - f64::from(origin.z),
                    );
                    self.motion.position += shift;
                    self.player.position = self.motion.position.as_vec3();
                    self.camera.rebase(shift);
                    self.origin = origin;
                    changed = true;
                }
            } else {
                self.stream_errors.push(format!(
                    "chunk {}, {} cannot be used as a block origin",
                    window.0.x, window.0.z
                ));
            }
            let outside: Vec<_> = self
                .world
                .chunks
                .keys()
                .copied()
                .filter(|pos| !within_window(*pos, window))
                .collect();
            let restore: Vec<_> = self
                .parked_chunks
                .keys()
                .copied()
                .filter(|pos| within_window(*pos, window))
                .collect();
            let old_missing = self
                .world
                .missing_chunks
                .iter()
                .any(|pos| !within_window(*pos, window));
            if !outside.is_empty() || !restore.is_empty() || old_missing {
                let mut restored_ticks = Vec::new();
                let unsaved: BTreeSet<_> = outside
                    .iter()
                    .copied()
                    .filter(|pos| self.has_unsaved_chunk_changes(*pos))
                    .collect();
                let retained_entities: BTreeSet<_> = outside
                    .iter()
                    .copied()
                    .filter(|pos| self.retain_entity_chunk(*pos))
                    .collect();
                let world = Arc::make_mut(&mut self.world);
                for pos in outside {
                    let chunk = world
                        .chunks
                        .remove(&pos)
                        .expect("resident chunk was checked");
                    if unsaved.contains(&pos)
                        || self
                            .respawn_window
                            .is_some_and(|extra| within_window(pos, extra))
                    {
                        self.parked_chunks.insert(pos, chunk);
                        respawn_changed = true;
                    } else {
                        if !retained_entities.contains(&pos)
                            && self.entity_chunks.remove(&pos).is_some()
                        {
                            respawn_changed = true;
                        }
                        self.entity_collision_cache.remove(&pos);
                        if self.spawn_heightmaps.remove(&pos).is_some() {
                            respawn_changed = true;
                        }
                    }
                    changed = true;
                }
                for pos in restore {
                    let chunk = self
                        .parked_chunks
                        .remove(&pos)
                        .expect("parked chunk was checked");
                    world.chunks.insert(pos, chunk);
                    world.missing_chunks.remove(&pos);
                    if self.fluid_chunks_seen.insert(pos) {
                        restored_ticks.push((
                            pos,
                            world
                                .saved_fluid_ticks
                                .get(&pos)
                                .cloned()
                                .unwrap_or_default(),
                            world
                                .saved_block_ticks
                                .get(&pos)
                                .cloned()
                                .unwrap_or_default(),
                        ));
                    }
                    respawn_changed = true;
                    changed = true;
                }
                world
                    .missing_chunks
                    .retain(|pos| within_window(*pos, window));
                for (pos, ticks, sand_ticks) in restored_ticks {
                    self.enqueue_saved_fluids(pos, &ticks);
                    self.enqueue_saved_sand(&sand_ticks);
                    self.enqueue_saved_buttons(&sand_ticks);
                    self.enqueue_saved_soil(&sand_ticks);
                }
            }
        }
        let unused: Vec<_> = self
            .parked_chunks
            .keys()
            .copied()
            .filter(|pos| {
                !self.has_unsaved_chunk_changes(*pos)
                    && !self
                        .respawn_window
                        .is_some_and(|window| within_window(*pos, window))
                    && !self
                        .stream_window
                        .is_some_and(|window| within_window(*pos, window))
            })
            .collect();
        for pos in unused {
            self.parked_chunks.remove(&pos);
            if !self.retain_entity_chunk(pos) {
                self.entity_chunks.remove(&pos);
            }
            self.entity_collision_cache.remove(&pos);
            self.spawn_heightmaps.remove(&pos);
            if !self.fluid_chunks_seen.contains(&pos) {
                let world = Arc::make_mut(&mut self.world);
                world.saved_fluid_ticks.remove(&pos);
                world.saved_block_ticks.remove(&pos);
            }
            respawn_changed = true;
        }
        while let Some(IncomingChunk {
            pos,
            world: source,
            entities,
            heightmaps,
        }) = self.incoming.pop_front()
        {
            let primary = self
                .stream_window
                .is_none_or(|window| within_window(pos, window));
            let respawn = self
                .respawn_window
                .is_some_and(|window| within_window(pos, window));
            if !primary && !respawn {
                continue;
            }
            match source {
                None => {
                    if let Some(data) = entities {
                        if (self.world.chunks.contains_key(&pos)
                            || self.parked_chunks.contains_key(&pos))
                            && matches!(
                                self.entity_chunks.get(&pos),
                                None | Some(SavedEntityData::Unavailable)
                            )
                            && matches!(data, SavedEntityData::Empty | SavedEntityData::Present(_))
                        {
                            match validate_entity_chunk(pos, &data) {
                                Ok(()) => {
                                    self.entity_chunks.insert(pos, data);
                                    self.entity_collision_cache.insert(pos, OnceLock::new());
                                    respawn_changed = true;
                                    if self.world.chunks.contains_key(&pos) {
                                        changed = true;
                                    }
                                }
                                Err(error) => self
                                    .stream_errors
                                    .push(format!("entity chunk {}, {}: {error:#}", pos.x, pos.z)),
                            }
                        }
                        continue;
                    }
                    if primary
                        && !self.world.chunks.contains_key(&pos)
                        && !self.parked_chunks.contains_key(&pos)
                    {
                        Arc::make_mut(&mut self.world).missing_chunks.insert(pos);
                    }
                }
                Some(source) => {
                    if self.world.chunks.contains_key(&pos) || self.parked_chunks.contains_key(&pos)
                    {
                        // Block data may already be resident after an earlier
                        // entity read failed. Accept a later successful entity
                        // read without reimporting or replacing the block chunk.
                        if matches!(
                            self.entity_chunks.get(&pos),
                            None | Some(SavedEntityData::Unavailable)
                        ) && let Some(
                            data @ (SavedEntityData::Empty | SavedEntityData::Present(_)),
                        ) = entities
                        {
                            match validate_entity_chunk(pos, &data) {
                                Ok(()) => {
                                    self.entity_chunks.insert(pos, data);
                                    self.entity_collision_cache.insert(pos, OnceLock::new());
                                    respawn_changed = true;
                                    if self.world.chunks.contains_key(&pos) {
                                        changed = true;
                                    }
                                }
                                Err(error) => self
                                    .stream_errors
                                    .push(format!("entity chunk {}, {}: {error:#}", pos.x, pos.z)),
                            }
                        }
                        continue;
                    }
                    let old_count = self.world.state_count();
                    let saved_ticks = source
                        .saved_fluid_ticks
                        .get(&pos)
                        .cloned()
                        .unwrap_or_default();
                    let saved_sand_ticks = source
                        .saved_block_ticks
                        .get(&pos)
                        .cloned()
                        .unwrap_or_default();
                    let mut entity_data = entities.unwrap_or(SavedEntityData::Unavailable);
                    if let Err(error) = validate_entity_chunk(pos, &entity_data) {
                        self.stream_errors
                            .push(format!("entity chunk {}, {}: {error:#}", pos.x, pos.z));
                        entity_data = SavedEntityData::Unavailable;
                    }
                    let heightmaps = heightmaps.and_then(|maps| {
                        if let Err(error) = self.validate_spawn_heightmaps(&maps) {
                            self.stream_errors
                                .push(format!("heightmap chunk {}, {}: {error:#}", pos.x, pos.z));
                            None
                        } else {
                            Some(maps)
                        }
                    });
                    match Arc::make_mut(&mut self.world).import_chunk(&source, pos) {
                        Ok(true) => {
                            // Preserve the original source of already-simulated
                            // entities even if a later entity read fails.
                            if !matches!(
                                self.entity_chunks.get(&pos),
                                Some(SavedEntityData::Present(_) | SavedEntityData::Empty)
                            ) {
                                self.entity_chunks.insert(pos, entity_data);
                            }
                            self.entity_collision_cache.insert(pos, OnceLock::new());
                            if let Some(maps) = heightmaps {
                                self.spawn_heightmaps.insert(pos, maps);
                            }
                            respawn_changed = true;
                            for (_, state) in self.world.states().skip(old_count) {
                                let shape = block_shape(state);
                                self.unsupported_states += usize::from(shape.is_none());
                                self.shapes.push(shape);
                                self.outlines.push(block_outline(state));
                                self.fluids
                                    .push(FluidState::from_block(state).ok().flatten());
                                if let Some(tags) = &self.heightmap_tags {
                                    self.heightmap_flags
                                        .push(crate::spawn_heightmaps::classify(state, tags).ok());
                                }
                            }
                            if primary {
                                if self.fluid_chunks_seen.insert(pos) {
                                    self.enqueue_saved_fluids(pos, &saved_ticks);
                                    self.enqueue_saved_sand(&saved_sand_ticks);
                                    self.enqueue_saved_buttons(&saved_sand_ticks);
                                    self.enqueue_saved_soil(&saved_sand_ticks);
                                }
                                changed = true;
                            } else {
                                let chunk = Arc::make_mut(&mut self.world)
                                    .chunks
                                    .remove(&pos)
                                    .expect("imported respawn chunk");
                                self.parked_chunks.insert(pos, chunk);
                                respawn_changed = true;
                            }
                        }
                        Ok(false) => {}
                        Err(error) => self
                            .stream_errors
                            .push(format!("chunk {}, {}: {error}", pos.x, pos.z)),
                    }
                }
            }
        }
        if changed {
            self.revision += 1;
        }
        if changed || self.tick == 0 {
            self.dropped_items
                .set_active_chunks(self.world.chunks.keys().copied().collect());
            self.experience_orbs
                .set_active_chunks(self.world.chunks.keys().copied().collect());
            self.activate_saved_items();
            self.activate_saved_orbs();
            self.activate_saved_falling_sand();
        }
        if respawn_changed {
            self.respawn_revision += 1;
        }
    }

    fn activate_saved_items(&mut self) {
        let pending: Vec<_> = self
            .world
            .chunks
            .keys()
            .copied()
            .filter(|pos| !self.item_chunks_seen.contains(pos))
            .filter(|pos| {
                matches!(
                    self.entity_chunks.get(pos),
                    Some(SavedEntityData::Empty | SavedEntityData::Present(_))
                )
            })
            .collect();
        for pos in pending {
            self.item_chunks_seen.insert(pos);
            let items = match self.saved_items_in_chunk(pos) {
                Ok(Some(items)) => items,
                Ok(None) => continue,
                Err(error) => {
                    self.stream_errors.push(format!(
                        "saved items in chunk {}, {}: {error:#}",
                        pos.x, pos.z
                    ));
                    continue;
                }
            };
            if let Some(uuid) = items
                .iter()
                .filter_map(|item| item.uuid)
                .find(|uuid| self.saved_item_uuids.contains_key(uuid))
            {
                self.stream_errors.push(format!(
                    "duplicate saved item UUID {uuid:?} in chunk {}, {}",
                    pos.x, pos.z
                ));
                continue;
            }
            for item in items {
                if let Some(uuid) = item.uuid {
                    self.saved_item_uuids.insert(uuid, pos);
                }
                self.dropped_items.spawn_saved(item);
            }
        }
    }

    fn activate_saved_orbs(&mut self) {
        let pending: Vec<_> = self
            .world
            .chunks
            .keys()
            .copied()
            .filter(|pos| !self.orb_chunks_seen.contains(pos))
            .filter(|pos| {
                matches!(
                    self.entity_chunks.get(pos),
                    Some(SavedEntityData::Empty | SavedEntityData::Present(_))
                )
            })
            .collect();
        for pos in pending {
            self.orb_chunks_seen.insert(pos);
            let Some(SavedEntityData::Present(chunk)) = self.entity_chunks.get(&pos) else {
                continue;
            };
            for entity in chunk.entities() {
                let saved = match crate::saved_orb::read(entity) {
                    Ok(Some(saved)) => saved,
                    Ok(None) => continue,
                    Err(error) => {
                        self.stream_errors.push(format!(
                            "saved experience orb in chunk {}, {}: {error:#}",
                            pos.x, pos.z
                        ));
                        continue;
                    }
                };
                if entity.get("Passengers").is_some() {
                    self.stream_errors.push(format!(
                        "saved experience orb has unsupported passengers in chunk {}, {}",
                        pos.x, pos.z
                    ));
                    continue;
                }
                if let Some(uuid) = saved.uuid {
                    if self.saved_orb_uuids.contains_key(&uuid) {
                        self.stream_errors.push(format!(
                            "duplicate saved experience-orb UUID {uuid:?} in chunk {}, {}",
                            pos.x, pos.z
                        ));
                        continue;
                    }
                    self.saved_orb_uuids.insert(uuid, pos);
                }
                self.experience_orbs.spawn_saved(saved);
            }
        }
    }

    fn activate_saved_falling_sand(&mut self) {
        let pending: Vec<_> = self
            .world
            .chunks
            .keys()
            .copied()
            .filter(|pos| !self.falling_chunks_seen.contains(pos))
            .filter(|pos| {
                matches!(
                    self.entity_chunks.get(pos),
                    Some(SavedEntityData::Empty | SavedEntityData::Present(_))
                )
            })
            .collect();
        for pos in pending {
            self.falling_chunks_seen.insert(pos);
            let chunk = match self.entity_chunks.get(&pos) {
                Some(SavedEntityData::Present(chunk)) => Arc::clone(chunk),
                _ => continue,
            };
            for entity in chunk.entities() {
                let saved = match crate::saved_falling_block::read(entity) {
                    Ok(Some(saved)) => saved,
                    Ok(None) => continue,
                    Err(error) => {
                        self.stream_errors.push(format!(
                            "saved falling block in chunk {}, {}: {error:#}",
                            pos.x, pos.z
                        ));
                        continue;
                    }
                };
                let kind = match FallingKind::from_id(&saved.block.name) {
                    Some(kind) => kind,
                    None => {
                        self.stream_errors.push(format!(
                            "unsupported falling block {} in chunk {}, {}",
                            saved.block.name, pos.x, pos.z
                        ));
                        continue;
                    }
                };
                if !saved.block.properties.is_empty()
                    || entity.get("TileEntityData").is_some()
                    || entity.get("Passengers").is_some()
                    || entity
                        .get("HurtEntities")
                        .and_then(crate::player_data::numeric_byte)
                        .is_some_and(|v| v != 0)
                {
                    self.stream_errors.push(format!(
                        "unsupported saved falling block in chunk {}, {}",
                        pos.x, pos.z
                    ));
                    continue;
                }
                if let Some(uuid) = saved.uuid {
                    if self.saved_falling_uuids.contains_key(&uuid) {
                        self.stream_errors.push(format!(
                            "duplicate saved falling-block UUID {uuid:?} in chunk {}, {}",
                            pos.x, pos.z
                        ));
                        continue;
                    }
                    self.saved_falling_uuids.insert(uuid, pos);
                }
                self.falling_sand.push(FallingSand {
                    motion: crate::falling_block::AirborneMotion {
                        position: saved.position,
                        velocity: saved.velocity,
                        age: saved.time,
                        no_gravity: saved.no_gravity,
                    },
                    previous: saved.position,
                    kind,
                    drop_item: saved.drop_item,
                    cancel_drop: saved.cancel_drop,
                    on_ground: saved.on_ground,
                    source: Some(saved.source),
                });
            }
        }
    }

    pub fn unsupported_states(&self) -> usize {
        self.unsupported_states
    }

    pub fn unsupported_outlines(&self) -> usize {
        self.outlines.iter().filter(|shape| shape.is_none()).count()
    }

    fn movement_below(&self) -> IVec3 {
        let support = self
            .ground_support
            .and_then(|absolute| subtract(absolute, self.origin));
        crate::movement::movement_on_block(self.motion.position, support, |pos| {
            match cell(&self.world, self.origin, self.bounds, pos, self.states[0]) {
                Cell::State(id) => self.world.state(id).is_some_and(|state| {
                    state.name.ends_with("_wall") || state.name.ends_with("_fence_gate")
                }),
                Cell::Missing => false,
            }
        })
    }

    fn friction_below(&self) -> f32 {
        let pos = self.movement_below();
        match cell(&self.world, self.origin, self.bounds, pos, self.states[0]) {
            Cell::State(id) => self
                .world
                .state(id)
                .map_or(0.6, |state| crate::physics::ground_friction(&state.name)),
            Cell::Missing => 0.6,
        }
    }

    fn block_jump_factor(&self) -> f32 {
        let factor = |pos| match cell(&self.world, self.origin, self.bounds, pos, self.states[0]) {
            Cell::State(id) => self
                .world
                .state(id)
                .map_or(1.0, |state| crate::physics::block_jump_factor(&state.name)),
            Cell::Missing => 1.0,
        };
        let at_feet = factor(self.motion.position.floor().as_ivec3());
        if at_feet == 1.0 {
            factor(self.movement_below())
        } else {
            at_feet
        }
    }

    fn placement_id(&mut self, block: Block) -> BlockStateId {
        match block {
            Block::Air => self.states[0],
            Block::Grass => self.states[1],
            Block::Dirt => self.states[2],
            Block::Stone => self.states[3],
            Block::StoneBricks => self.states[4],
            Block::Wood(wood, axis) => {
                self.intern_block(state(wood.id(), &[("axis", axis.name())]))
            }
            Block::Podzol | Block::Mycelium => {
                self.intern_block(state(block.item_id().unwrap(), &[("snowy", "false")]))
            }
            Block::Farmland(moisture) => self.intern_block(state(
                "minecraft:farmland",
                &[("moisture", &moisture.to_string())],
            )),
            Block::Cobblestone
            | Block::Planks(_)
            | Block::CraftingTable
            | Block::CoarseDirt
            | Block::RootedDirt
            | Block::Sand
            | Block::RedSand
            | Block::Gravel
            | Block::DirtPath => {
                self.intern_block(state(block.item_id().expect("placeable block"), &[]))
            }
        }
    }

    fn soil_cover(&self, soil: SoilKind, name: &str) -> Option<bool> {
        if soil == SoilKind::Path && is_vanilla_fence_gate(name) {
            return Some(false);
        }
        if soil == SoilKind::Farmland
            && self
                .farmland_maintainers
                .as_ref()
                .is_some_and(|tag| tag.contains(name))
        {
            return Some(false);
        }
        let solid = known_soil_cover(name)?;
        if solid && soil == SoilKind::Farmland && self.farmland_maintainers.is_none() {
            return None;
        }
        Some(solid)
    }

    fn soil_cover_state(&self, soil: SoilKind, state: &BlockState) -> Option<bool> {
        if let Some(result) = self.soil_cover(soil, &state.name) {
            return Some(result);
        }
        if !self.vanilla_block_ids.as_ref()?.contains(&state.name) {
            return None;
        }
        let solid = native_world::collision::slab_stair_solid(state)?;
        if solid && soil == SoilKind::Farmland && self.farmland_maintainers.is_none() {
            return None;
        }
        Some(solid)
    }

    fn placement_block(&mut self, relative: IVec3, block: Block) -> Option<Block> {
        let Some(kind) = SoilKind::from_block(block) else {
            return Some(block);
        };
        let above = add(relative, IVec3::Y)?;
        let Cell::State(id) = cell(&self.world, self.origin, self.bounds, above, self.states[0])
        else {
            return None;
        };
        let state = self.world.state(id)?;
        let blocked = self.soil_cover_state(kind, state)?;
        if kind == SoilKind::Path && blocked {
            let absolute = add(self.origin, relative)?;
            self.push_entities_from_soil(relative, absolute);
        }
        Some(placed_soil(block, blocked))
    }

    /// Test candidate supports in Java's view-dependent order. An unknown
    /// earlier support cannot safely be skipped in favour of a later one.
    fn place_stone_button(&mut self, hit: ShapeRayHit) -> Option<crate::inventory::PlayerSlot> {
        use crate::inventory::PlayerSlot;
        let main = PlayerSlot::Storage(self.inventory.selected());
        let hand = if self.inventory.slot(main).is_some() {
            main
        } else {
            PlayerSlot::Offhand
        };
        let held = self.inventory.slot(hand)?;
        if held.id() != "minecraft:stone_button" || held.max_count().is_none() {
            return None;
        }
        let Cell::State(clicked_id) = cell(
            &self.world,
            self.origin,
            self.bounds,
            hit.position,
            self.states[0],
        ) else {
            return None;
        };
        let replace_clicked = stone_button_can_replace(self.world.state(clicked_id)?)?;
        let target = if replace_clicked {
            hit.position
        } else {
            add(hit.position, hit.face)?
        };
        let Cell::State(target_id) = cell(
            &self.world,
            self.origin,
            self.bounds,
            target,
            self.states[0],
        ) else {
            return None;
        };
        if stone_button_can_replace(self.world.state(target_id)?) != Some(true) {
            return None;
        }
        for direction in button_placement_directions(
            self.player.yaw,
            self.player.pitch,
            hit.face,
            replace_clicked,
        )? {
            let support = add(target, direction)?;
            let Cell::State(support_id) = cell(
                &self.world,
                self.origin,
                self.bounds,
                support,
                self.states[0],
            ) else {
                return None;
            };
            let support = self.world.state(support_id)?;
            match button_support_face(support, -direction)? {
                false => continue,
                true => {
                    let block = stone_button_state(-direction, self.player.yaw)?;
                    let id = self.intern_block(block);
                    return self.set(target, id).then_some(hand);
                }
            }
        }
        None
    }

    fn ray_hit(&self) -> Option<ShapeRayHit> {
        raycast_shapes(
            self.player.eye(),
            self.player.direction(),
            5.0,
            |relative| match cell(
                &self.world,
                self.origin,
                self.bounds,
                relative,
                self.states[0],
            ) {
                Cell::Missing => None,
                Cell::State(id) => Some(
                    self.outlines
                        .get(id.0 as usize)
                        .and_then(Option::as_deref)
                        .unwrap_or_else(|| unknown_barrier()),
                ),
            },
        )
    }

    fn mining_target(&self) -> Option<mining::Candidate> {
        let hit = self.ray_hit()?;
        let Cell::State(id) = cell(
            &self.world,
            self.origin,
            self.bounds,
            hit.position,
            self.states[0],
        ) else {
            return None;
        };
        let block = self.world.state(id)?;
        if mining::requires_game_master_permission(&block.name)
            && (self.game_mode != GameMode::Creative || !self.game_master_permission)
        {
            return None;
        }
        Some(mining::Candidate {
            position: add(self.origin, hit.position)?,
            material: mining::Material::from_block_name(&block.name),
        })
    }

    fn finish_breaking(&mut self, absolute: IVec3) {
        let Some(relative) = subtract(absolute, self.origin) else {
            return;
        };
        let Cell::State(id) = cell(
            &self.world,
            self.origin,
            self.bounds,
            relative,
            self.states[0],
        ) else {
            return;
        };
        let Some(name) = self.world.state(id).map(|state| state.name.clone()) else {
            return;
        };
        if self.set(relative, self.states[0]) {
            let hand = self.inventory.slot(crate::inventory::PlayerSlot::Storage(
                self.inventory.selected(),
            ));
            let damage = mining::Material::from_block_name(&name)
                .and_then(|material| mining::mining_properties(material, hand))
                .map_or(0, |properties| properties.damage_per_block);
            self.dropped_items.drop_block(
                absolute,
                &name,
                hand,
                self.game_mode,
                self.block_drops,
                self.loot.as_deref(),
            );
            if self.game_mode != GameMode::Creative && damage != 0 {
                Arc::make_mut(&mut self.inventory)
                    .damage_selected_tool(damage, &mut self.level_random);
            }
        }
    }

    fn use_tool(&mut self, relative: IVec3, face: IVec3) -> bool {
        let Cell::State(id) = cell(
            &self.world,
            self.origin,
            self.bounds,
            relative,
            self.states[0],
        ) else {
            return false;
        };
        let Some(block) = self.world.state(id) else {
            return false;
        };
        let source = TransformState {
            name: block.name.clone(),
            properties: block.properties.clone(),
        };
        if supported_transform_state(&source).is_none() {
            return false;
        }
        let Some(face_id) = face_name(face) else {
            return false;
        };
        for (hand, kind) in tool_hands(&self.inventory, self.sneak)
            .into_iter()
            .flatten()
        {
            let Some(transforms) = &self.tool_transforms[kind.index()] else {
                return true;
            };
            let result = transforms.apply_at(&source, face_id, |offset| {
                let relative = add(relative, IVec3::from_array(offset))?;
                let Cell::State(id) = cell(
                    &self.world,
                    self.origin,
                    self.bounds,
                    relative,
                    self.states[0],
                ) else {
                    return None;
                };
                self.world.state(id).map(|state| TransformState {
                    name: state.name.clone(),
                    properties: state.properties.clone(),
                })
            });
            let result = match result {
                Ok(Some(result)) => result,
                Ok(None) => continue,
                Err(_) => return true,
            };
            let Some(block) = supported_transform_state(&result.state) else {
                return true;
            };
            if result.update_from_neighbors
                && matches!(block, Block::Grass | Block::Podzol | Block::Mycelium)
            {
                return true;
            }
            if result.update_from_neighbors && matches!(block, Block::DirtPath | Block::Farmland(_))
            {
                let Some(above) = add(relative, IVec3::Y) else {
                    return true;
                };
                let Cell::State(above) =
                    cell(&self.world, self.origin, self.bounds, above, self.states[0])
                else {
                    return true;
                };
                if self.world.state(above).and_then(|state| {
                    self.soil_cover_state(SoilKind::from_block(block).unwrap(), state)
                }) != Some(false)
                {
                    return true;
                }
            }
            let Some(drops) =
                interaction_drops(&result, self.loot.as_deref(), &self.inventory, hand)
            else {
                return true;
            };
            let Some(absolute) = add(self.origin, relative) else {
                return true;
            };
            let target = self.intern_block(BlockState {
                name: result.state.name,
                properties: result.state.properties,
            });
            if target != id && !self.set(relative, target) {
                return true;
            }
            if let Some((_, strategy)) = &result.loot {
                self.dropped_items.drop_interaction(
                    absolute,
                    face,
                    *strategy,
                    drops,
                    self.block_drops,
                );
            }
            if self.game_mode != GameMode::Creative && result.damage != 0 {
                Arc::make_mut(&mut self.inventory).damage_held_tool(
                    hand,
                    result.damage,
                    &mut self.level_random,
                );
            }
            let swing = if hand == crate::inventory::PlayerSlot::Offhand {
                crate::animation::Hand::Off
            } else {
                crate::animation::Hand::Main
            };
            self.first_person.swing.start(swing);
            if self.inventory.slot(hand).is_some() {
                self.first_person.used(swing);
            }
            return true;
        }
        false
    }

    fn set(&mut self, relative: IVec3, id: BlockStateId) -> bool {
        let Some(pos) = add(self.origin, relative) else {
            return false;
        };
        if pos.y < self.bounds.min_y || pos.y >= self.bounds.max_y {
            return false;
        }
        let chunk_pos = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        let Some(chunk) = self.world.chunks.get(&chunk_pos) else {
            return false;
        };
        if chunk.status.as_deref() != Some("minecraft:full") {
            return false;
        }
        let Ok(section_y) = i8::try_from(pos.y.div_euclid(16)) else {
            return false;
        };
        let index = pos.x.rem_euclid(16) as usize
            + pos.z.rem_euclid(16) as usize * 16
            + pos.y.rem_euclid(16) as usize * 256;
        let original = chunk
            .sections
            .get(&section_y)
            .and_then(|section| section.blocks.as_ref())
            .map(|blocks| blocks[index]);
        let old = original.unwrap_or(self.states[0]);
        if old == id {
            return false;
        }
        let original_state = original.and_then(|old| self.world.state(old).cloned());
        let Some(replacement_state) = self.world.state(id).cloned() else {
            return false;
        };
        let world = Arc::make_mut(&mut self.world);
        let chunk = world.chunks.get_mut(&chunk_pos).expect("chunk was checked");
        let chunk = Arc::make_mut(chunk);
        let section = chunk.sections.entry(section_y).or_insert_with(|| {
            Arc::new(Section {
                blocks: None,
                biomes: None,
                block_light: None,
                sky_light: None,
            })
        });
        let section = Arc::make_mut(section);
        let blocks = section
            .blocks
            .get_or_insert_with(|| Box::new([self.states[0]; BLOCK_COUNT]));
        blocks[index] = id;
        world.record_block_edit(
            BlockPosition {
                x: pos.x,
                y: pos.y,
                z: pos.z,
            },
            original_state,
            replacement_state,
        );
        if let Some(mut maps) = self.spawn_heightmaps.remove(&chunk_pos) {
            let update = (|| -> anyhow::Result<()> {
                use anyhow::Context;
                let flags = self
                    .heightmap_flags
                    .get(id.0 as usize)
                    .copied()
                    .flatten()
                    .context("new block heightmap behavior is unavailable")?;
                let air = self
                    .heightmap_air
                    .context("air heightmap behavior is unavailable")?;
                let chunk = self
                    .world
                    .chunks
                    .get(&chunk_pos)
                    .context("edited heightmap chunk is missing")?;
                maps.update_after_change(pos.x, pos.y, pos.z, flags, |x, y, z| {
                    crate::spawn_heightmaps::block_flags(chunk, &self.heightmap_flags, air, x, y, z)
                })
            })();
            match update {
                Ok(()) => {
                    self.spawn_heightmaps.insert(chunk_pos, maps);
                }
                Err(error) => self.stream_errors.push(format!(
                    "heightmap chunk {}, {}: {error:#}",
                    chunk_pos.x, chunk_pos.z
                )),
            }
        }
        self.block_edits
            .entry(chunk_pos)
            .or_default()
            .entry((pos.x, pos.y, pos.z))
            .and_modify(|edit| edit.current = id)
            .or_insert(RecordedBlockEdit {
                original,
                current: id,
            });
        self.dirty_chunks.insert(chunk_pos);
        self.revision += 1;
        self.block_notifications.push_back((relative, old, id));
        self.notify_fluid_neighbors();
        true
    }

    fn fluid_settings(&self) -> fluid_sim::Settings {
        fluid_sim::Settings {
            fast_lava: self.fast_lava,
            water_source_conversion: self.water_source_conversion,
            lava_source_conversion: self.lava_source_conversion,
        }
    }

    /// Other fluid-bearing blocks have separate placement and update rules.
    fn ordinary_fluid(&self, id: BlockStateId) -> Option<FluidState> {
        let state = self.world.state(id)?;
        if !matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
            return None;
        }
        self.fluids.get(id.0 as usize).copied().flatten()
    }

    fn schedule_fluid(&mut self, relative: IVec3, state: FluidState, delay: u64) {
        let Some(absolute) = add(self.origin, relative) else {
            return;
        };
        let key = FluidTickKey {
            x: absolute.x,
            y: absolute.y,
            z: absolute.z,
            kind: state.kind,
            source: state.source,
        };
        if !self.fluid_pending.contains(&key) {
            self.mark_tick_chunk_dirty(key.x, key.z);
        }
        let subtick = i64::try_from(self.fluid_sequence).unwrap_or(i64::MAX);
        self.insert_fluid_tick(key, i128::from(self.tick) + i128::from(delay), 0, subtick);
    }

    fn insert_fluid_tick(&mut self, key: FluidTickKey, due: i128, priority: i32, subtick: i64) {
        if !self.fluid_pending.insert(key) {
            return;
        }
        self.insert_fluid_tick_unchecked(key, due, priority, subtick);
    }

    fn insert_fluid_tick_unchecked(
        &mut self,
        key: FluidTickKey,
        due: i128,
        priority: i32,
        subtick: i64,
    ) {
        let order = (due, priority, subtick, self.fluid_sequence);
        self.fluid_sequence = self.fluid_sequence.wrapping_add(1);
        self.fluid_ticks.insert(order, ScheduledFluid { key });
    }

    fn enqueue_saved_fluids(&mut self, chunk: ChunkPos, ticks: &[SavedFluidTick]) {
        let mut unsupported = 0;
        for (index, tick) in ticks.iter().enumerate() {
            let (kind, source) = match tick.fluid.as_str() {
                "minecraft:water" | "water" => (FluidKind::Water, true),
                "minecraft:flowing_water" | "flowing_water" => (FluidKind::Water, false),
                "minecraft:lava" | "lava" => (FluidKind::Lava, true),
                "minecraft:flowing_lava" | "flowing_lava" => (FluidKind::Lava, false),
                _ => {
                    unsupported += 1;
                    continue;
                }
            };
            let [x, y, z] = tick.position;
            self.tick_chunks_needing_dirty.insert(chunk);
            let key = FluidTickKey {
                x,
                y,
                z,
                kind,
                source,
            };
            let subtick = i64::try_from(index).unwrap_or(i64::MAX)
                - i64::try_from(ticks.len()).unwrap_or(i64::MAX);
            // Java's saved tick container keeps duplicate stored entries when
            // unpacking, while the uniqueness set still blocks new schedules.
            self.fluid_pending.insert(key);
            self.insert_fluid_tick_unchecked(
                key,
                i128::from(self.tick) + i128::from(tick.delay),
                tick.priority.clamp(-3, 3),
                subtick,
            );
        }
        if unsupported > 0 {
            self.stream_errors.push(format!(
                "chunk {}, {}: {unsupported} unsupported saved fluid ticks were not simulated",
                chunk.x, chunk.z
            ));
        }
    }

    fn enqueue_saved_sand(&mut self, ticks: &[SavedBlockTick]) {
        for (index, tick) in ticks.iter().enumerate() {
            let Some(kind) = FallingKind::from_id(&tick.block) else {
                continue;
            };
            let [x, y, z] = tick.position;
            let key = SandTickKey { x, y, z, kind };
            self.tick_chunks_needing_dirty.insert(ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            });
            let subtick = i64::try_from(index).unwrap_or(i64::MAX)
                - i64::try_from(ticks.len()).unwrap_or(i64::MAX);
            self.sand_pending.insert(key);
            let sequence = self.block_tick_sequence;
            self.block_tick_sequence = self.block_tick_sequence.wrapping_add(1);
            let order = (
                i128::from(self.tick) + i128::from(tick.delay),
                tick.priority.clamp(-3, 3),
                subtick,
                sequence,
            );
            self.sand_ticks.insert(order, key);
        }
    }

    fn enqueue_saved_buttons(&mut self, ticks: &[SavedBlockTick]) {
        for (index, tick) in ticks.iter().enumerate() {
            if !matches!(
                tick.block.as_str(),
                "minecraft:stone_button" | "stone_button"
            ) {
                continue;
            }
            let [x, y, z] = tick.position;
            let key = (x, y, z);
            self.tick_chunks_needing_dirty.insert(ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            });
            let subtick = i64::try_from(index).unwrap_or(i64::MAX)
                - i64::try_from(ticks.len()).unwrap_or(i64::MAX);
            self.button_pending.insert(key);
            let sequence = self.block_tick_sequence;
            self.block_tick_sequence = self.block_tick_sequence.wrapping_add(1);
            let order = (
                i128::from(self.tick) + i128::from(tick.delay),
                tick.priority.clamp(-3, 3),
                subtick,
                sequence,
            );
            self.button_ticks.insert(order, key);
        }
    }

    fn enqueue_saved_soil(&mut self, ticks: &[SavedBlockTick]) {
        for (index, tick) in ticks.iter().enumerate() {
            let Some(soil) = SoilKind::from_name(&tick.block) else {
                continue;
            };
            let [x, y, z] = tick.position;
            self.tick_chunks_needing_dirty.insert(ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            });
            let subtick = i64::try_from(index).unwrap_or(i64::MAX)
                - i64::try_from(ticks.len()).unwrap_or(i64::MAX);
            self.soil_pending.insert((x, y, z, soil));
            let sequence = self.block_tick_sequence;
            self.block_tick_sequence = self.block_tick_sequence.wrapping_add(1);
            self.soil_ticks.insert(
                (
                    i128::from(self.tick) + i128::from(tick.delay),
                    tick.priority.clamp(-3, 3),
                    subtick,
                    sequence,
                ),
                (IVec3::new(x, y, z), soil),
            );
        }
    }

    fn schedule_stone_button(&mut self, relative: IVec3) {
        let Some(absolute) = add(self.origin, relative) else {
            return;
        };
        let key = (absolute.x, absolute.y, absolute.z);
        if !self.button_pending.insert(key) {
            return;
        }
        self.mark_tick_chunk_dirty(key.0, key.2);
        let sequence = self.block_tick_sequence;
        self.block_tick_sequence = self.block_tick_sequence.wrapping_add(1);
        self.button_ticks.insert(
            (
                i128::from(self.tick) + 20,
                0,
                i64::try_from(sequence).unwrap_or(i64::MAX),
                sequence,
            ),
            key,
        );
    }

    fn set_stone_button_power(&mut self, relative: IVec3, powered: bool) -> bool {
        let Cell::State(id) = cell(
            &self.world,
            self.origin,
            self.bounds,
            relative,
            self.states[0],
        ) else {
            return false;
        };
        let Some(old) = self.world.state(id) else {
            return false;
        };
        if old.name != "minecraft:stone_button" || block_outline(old).is_none() {
            return false;
        }
        if old.properties.get("powered").map(String::as_str)
            == Some(if powered { "true" } else { "false" })
        {
            return false;
        }
        let mut next = old.clone();
        next.properties
            .insert("powered".into(), powered.to_string());
        let id = self.intern_block(next);
        self.set(relative, id)
    }

    fn use_stone_button(&mut self, relative: IVec3) -> bool {
        let Cell::State(id) = cell(
            &self.world,
            self.origin,
            self.bounds,
            relative,
            self.states[0],
        ) else {
            return false;
        };
        let Some(block) = self.world.state(id) else {
            return false;
        };
        if block.name != "minecraft:stone_button" || block_outline(block).is_none() {
            return false;
        }
        if self.set_stone_button_power(relative, true) {
            self.schedule_stone_button(relative);
            self.first_person.swing.start(crate::animation::Hand::Main);
            self.push_button_sound(relative, true);
        }
        true
    }

    fn push_button_sound(&mut self, relative: IVec3, powered: bool) {
        if let Some(position) = add(self.origin, relative) {
            if self.button_sounds.len() == 4096 {
                self.button_sounds.pop_front();
            }
            self.button_sounds.push_back(ButtonSoundEvent {
                position,
                powered,
                tick: self.tick,
            });
        }
    }

    /// Consume transient sounds after update; repeated snapshot reads never replay them.
    pub fn take_button_sounds(&mut self) -> Vec<ButtonSoundEvent> {
        self.button_sounds.drain(..).collect()
    }

    fn advance_block_ticks(&mut self) {
        self.advance_scheduled_block_ticks(true, true, true);
    }

    #[cfg(test)]
    fn advance_sand_ticks(&mut self) {
        self.advance_scheduled_block_ticks(true, false, false);
    }

    #[cfg(test)]
    fn advance_soil_ticks(&mut self) {
        self.advance_scheduled_block_ticks(false, false, true);
    }

    fn advance_scheduled_block_ticks(
        &mut self,
        process_sand: bool,
        process_buttons: bool,
        process_soil: bool,
    ) {
        let revision = self.revision;
        if process_sand && self.sand_deferred_revision != Some(revision) {
            self.sand_ticks.append(&mut self.sand_deferred);
        }
        if process_buttons && self.button_deferred_revision != Some(revision) {
            self.button_ticks.append(&mut self.button_deferred);
        }
        if process_soil && self.soil_deferred_revision != Some(revision) {
            self.soil_ticks.append(&mut self.soil_deferred);
        }
        let mut deferred_sand = Vec::new();
        let mut deferred_buttons = Vec::new();
        let mut deferred_soil = Vec::new();
        let mut executed = 0;
        while executed < 65_536 {
            let sand_order = process_sand
                .then(|| self.sand_ticks.first_key_value().map(|(order, _)| *order))
                .flatten();
            let button_order = process_buttons
                .then(|| self.button_ticks.first_key_value().map(|(order, _)| *order))
                .flatten();
            let soil_order = process_soil
                .then(|| self.soil_ticks.first_key_value().map(|(order, _)| *order))
                .flatten();
            let Some((order, kind)) = [(sand_order, 0), (button_order, 1), (soil_order, 2)]
                .into_iter()
                .filter_map(|(order, kind)| order.map(|order| (order, kind)))
                .min()
            else {
                break;
            };
            if order.0 > i128::from(self.tick) {
                break;
            }
            if kind == 0 {
                let (order, key) = self
                    .sand_ticks
                    .pop_first()
                    .expect("selected sand tick remains queued");
                let Some(relative) = subtract(IVec3::new(key.x, key.y, key.z), self.origin) else {
                    deferred_sand.push((order, key));
                    continue;
                };
                let Cell::State(id) = cell(
                    &self.world,
                    self.origin,
                    self.bounds,
                    relative,
                    self.states[0],
                ) else {
                    deferred_sand.push((order, key));
                    continue;
                };
                self.mark_tick_chunk_dirty(key.x, key.z);
                if self
                    .world
                    .state(id)
                    .is_none_or(|state| state.name != key.kind.id())
                {
                    self.sand_pending.remove(&key);
                    executed += 1;
                    continue;
                }
                let Some(below) = add(relative, IVec3::NEG_Y) else {
                    deferred_sand.push((order, key));
                    continue;
                };
                let Some(free) = self.falling_support_is_free(below) else {
                    deferred_sand.push((order, key));
                    continue;
                };
                self.sand_pending.remove(&key);
                executed += 1;
                if free && key.y >= self.bounds.min_y {
                    let motion = crate::falling_block::AirborneMotion::from_block(IVec3::new(
                        key.x, key.y, key.z,
                    ));
                    if self.set(relative, self.states[0]) {
                        self.falling_sand.push(FallingSand {
                            previous: motion.position,
                            motion,
                            kind: key.kind,
                            drop_item: true,
                            cancel_drop: false,
                            on_ground: false,
                            source: None,
                        });
                    }
                }
            } else if kind == 1 {
                let (order, key) = self
                    .button_ticks
                    .pop_first()
                    .expect("selected button tick remains queued");
                let Some(relative) = subtract(IVec3::new(key.0, key.1, key.2), self.origin) else {
                    deferred_buttons.push((order, key));
                    continue;
                };
                if matches!(
                    cell(
                        &self.world,
                        self.origin,
                        self.bounds,
                        relative,
                        self.states[0]
                    ),
                    Cell::Missing
                ) {
                    deferred_buttons.push((order, key));
                    continue;
                }
                self.mark_tick_chunk_dirty(key.0, key.2);
                self.button_pending.remove(&key);
                executed += 1;
                if self.set_stone_button_power(relative, false) {
                    self.push_button_sound(relative, false);
                }
            } else {
                let (order, (absolute, soil)) = self
                    .soil_ticks
                    .pop_first()
                    .expect("selected soil tick remains queued");
                let Some(relative) = subtract(absolute, self.origin) else {
                    deferred_soil.push((order, (absolute, soil)));
                    continue;
                };
                let Cell::State(id) = cell(
                    &self.world,
                    self.origin,
                    self.bounds,
                    relative,
                    self.states[0],
                ) else {
                    deferred_soil.push((order, (absolute, soil)));
                    continue;
                };
                let matches_soil = self
                    .world
                    .state(id)
                    .and_then(|state| SoilKind::from_name(&state.name))
                    == Some(soil);
                let blocked = if matches_soil && soil == SoilKind::Farmland {
                    let Some(above) = add(relative, IVec3::Y) else {
                        self.soil_pending
                            .remove(&(absolute.x, absolute.y, absolute.z, soil));
                        executed += 1;
                        continue;
                    };
                    let Cell::State(above) =
                        cell(&self.world, self.origin, self.bounds, above, self.states[0])
                    else {
                        deferred_soil.push((order, (absolute, soil)));
                        continue;
                    };
                    let Some(blocked) = self
                        .world
                        .state(above)
                        .and_then(|state| self.soil_cover_state(SoilKind::Farmland, state))
                    else {
                        deferred_soil.push((order, (absolute, soil)));
                        continue;
                    };
                    blocked
                } else {
                    false
                };
                self.soil_pending
                    .remove(&(absolute.x, absolute.y, absolute.z, soil));
                self.mark_tick_chunk_dirty(absolute.x, absolute.z);
                executed += 1;
                if matches_soil && (soil == SoilKind::Path || blocked) {
                    self.push_entities_from_soil(relative, absolute);
                    self.set(relative, self.states[2]);
                }
            }
        }
        if process_soil {
            self.soil_deferred.extend(deferred_soil);
            self.soil_deferred_revision = Some(revision);
        }
        if process_sand {
            self.sand_deferred.extend(deferred_sand);
            // Block edits during this pass may have resolved an earlier deferred
            // tick. Compare against the revision at entry again on the next tick.
            self.sand_deferred_revision = Some(revision);
        }
        if process_buttons {
            self.button_deferred.extend(deferred_buttons);
            self.button_deferred_revision = Some(revision);
        }
    }

    fn push_entities_from_soil(&mut self, relative: IVec3, absolute: IVec3) {
        let bounds =
            crate::movement::player_bounds(self.motion.position, self.player.pose.height());
        if self.game_mode != GameMode::Spectator
            && let Some(delta) = native_world::movement::soil_expansion_push(bounds, relative)
        {
            self.motion.position.y += delta;
            self.player.position = self.motion.position.as_vec3();
            self.camera.snap_position(self.motion.position);
        }
        self.dropped_items.push_from_soil_expansion(absolute);
        self.experience_orbs.push_from_soil_expansion(absolute);
        for entity in &mut self.falling_sand {
            let at = entity.motion.position.floor().as_ivec3();
            if !self.world.chunks.contains_key(&ChunkPos {
                x: at.x.div_euclid(16),
                z: at.z.div_euclid(16),
            }) {
                continue;
            }
            if let Some(delta) =
                native_world::movement::soil_expansion_push(entity.motion.bounds(), absolute)
            {
                entity.motion.position.y += delta;
                entity.previous = entity.motion.position;
            }
        }
    }

    fn schedule_sand(&mut self, relative: IVec3, kind: FallingKind) {
        let Some(absolute) = add(self.origin, relative) else {
            return;
        };
        let key = SandTickKey {
            x: absolute.x,
            y: absolute.y,
            z: absolute.z,
            kind,
        };
        if !self.sand_pending.insert(key) {
            return;
        }
        self.mark_tick_chunk_dirty(key.x, key.z);
        let sequence = self.block_tick_sequence;
        self.block_tick_sequence = self.block_tick_sequence.wrapping_add(1);
        self.sand_ticks.insert(
            (
                i128::from(self.tick) + 2,
                0,
                i64::try_from(sequence).unwrap_or(i64::MAX),
                sequence,
            ),
            key,
        );
    }

    fn falling_sand_snapshot(&self) -> Arc<[crate::falling_block::FallingBlockVisual]> {
        self.falling_sand
            .iter()
            .filter(|entity| {
                let position = entity.motion.position.floor().as_ivec3();
                self.world.chunks.contains_key(&ChunkPos {
                    x: position.x.div_euclid(16),
                    z: position.z.div_euclid(16),
                })
            })
            .map(|entity| crate::falling_block::FallingBlockVisual {
                previous_position: entity.previous,
                position: entity.motion.position,
                kind: entity.kind,
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn falling_support_is_free(&self, relative: IVec3) -> Option<bool> {
        let Cell::State(id) = cell(
            &self.world,
            self.origin,
            self.bounds,
            relative,
            self.states[0],
        ) else {
            return None;
        };
        let state = self.world.state(id)?;
        let fire = self.falling_fire_tag.as_ref()?;
        crate::falling_block::known_free_below(&state.name, fire.contains(&state.name)).or_else(
            || {
                self.shapes
                    .get(id.0 as usize)
                    .and_then(Option::as_deref)
                    .filter(|shape| crate::falling_block::known_collision_support(shape))
                    .map(|_| false)
            },
        )
    }

    fn advance_falling_sand(&mut self) {
        let mut remaining = Vec::new();
        for mut entity in std::mem::take(&mut self.falling_sand) {
            entity.previous = entity.motion.position;
            let relative = entity.motion.position - self.origin.as_dvec3();
            let mut probe = entity.motion;
            probe.position = relative;
            let mut requested = probe.velocity;
            if !probe.no_gravity {
                requested.y -= 0.04;
            }
            let search = probe.bounds().expanded_towards(requested);
            let start = (search.min - DVec3::splat(1.0e-6)).floor().as_ivec3();
            let end = (search.max + DVec3::splat(1.0e-6)).floor().as_ivec3();
            let count = (i64::from(end.x) - i64::from(start.x) + 1)
                .checked_mul(i64::from(end.y) - i64::from(start.y) + 2)
                .and_then(|v| v.checked_mul(i64::from(end.z) - i64::from(start.z) + 1))
                .unwrap_or(i64::MAX);
            let mut complete = count > 0 && count <= 4096;
            if complete {
                'check: for x in start.x..=end.x {
                    for y in start.y.saturating_sub(1)..=end.y {
                        for z in start.z..=end.z {
                            let Cell::State(id) = cell(
                                &self.world,
                                self.origin,
                                self.bounds,
                                IVec3::new(x, y, z),
                                self.states[0],
                            ) else {
                                complete = false;
                                break 'check;
                            };
                            if self
                                .shapes
                                .get(id.0 as usize)
                                .and_then(Option::as_deref)
                                .is_none()
                                || (self.fluids.get(id.0 as usize).copied().flatten().is_some()
                                    && self.ordinary_fluid(id).is_none())
                            {
                                complete = false;
                                break 'check;
                            }
                        }
                    }
                }
            }
            if !complete {
                remaining.push(entity);
                continue;
            }
            let world = &self.world;
            let shapes = &self.shapes;
            let origin = self.origin;
            let bounds = self.bounds;
            let air = self.states[0];
            let template = entity.motion;
            let collision = entity.motion.tick_with_collision(|position, movement| {
                let mut local = template;
                local.position = position - origin.as_dvec3();
                crate::movement::resolve_box(local.bounds(), movement, false, 0.0, |p| {
                    match cell(world, origin, bounds, p, air) {
                        Cell::State(id) => shapes
                            .get(id.0 as usize)
                            .and_then(Option::as_deref)
                            .unwrap_or_else(|| unknown_barrier()),
                        Cell::Missing => unknown_barrier(),
                    }
                })
            });
            entity.on_ground = collision.on_ground;
            if collision.on_ground {
                if entity.cancel_drop {
                    continue;
                }
                let target = entity.motion.position.floor().as_ivec3();
                let Some(target_relative) = subtract(target, self.origin) else {
                    remaining.push(entity);
                    continue;
                };
                let Cell::State(current) = cell(
                    &self.world,
                    self.origin,
                    self.bounds,
                    target_relative,
                    self.states[0],
                ) else {
                    remaining.push(entity);
                    continue;
                };
                let can_replace = self.world.state(current).is_some_and(|state| {
                    matches!(
                        state.name.as_str(),
                        "minecraft:air"
                            | "minecraft:cave_air"
                            | "minecraft:void_air"
                            | "minecraft:fire"
                            | "minecraft:soul_fire"
                            | "minecraft:short_grass"
                            | "minecraft:fern"
                            | "minecraft:dead_bush"
                            | "minecraft:water"
                            | "minecraft:lava"
                    )
                });
                if can_replace {
                    let Some(below) = add(target_relative, IVec3::NEG_Y) else {
                        remaining.push(entity);
                        continue;
                    };
                    match self.falling_support_is_free(below) {
                        Some(true) => {
                            self.drop_falling_sand(&entity);
                            continue;
                        }
                        None => {
                            remaining.push(entity);
                            continue;
                        }
                        Some(false) => {}
                    }
                    let name = entity.kind.id();
                    let id = self.intern_block(state(name, &[]));
                    if self.set(target_relative, id) {
                        continue;
                    }
                } else if self
                    .shapes
                    .get(current.0 as usize)
                    .and_then(Option::as_deref)
                    .is_some_and(crate::falling_block::known_collision_support)
                {
                    self.drop_falling_sand(&entity);
                    continue;
                }
            }
            if !collision.on_ground
                && crate::falling_block::expires_in_air(
                    entity.motion.age,
                    entity.motion.position.floor().as_ivec3().y,
                    self.bounds.min_y,
                    self.bounds.max_y,
                )
            {
                self.drop_falling_sand(&entity);
                continue;
            }
            remaining.push(entity);
        }
        self.falling_sand = remaining;
    }

    fn drop_falling_sand(&mut self, entity: &FallingSand) {
        if self.entity_drops && entity.drop_item {
            let name = entity.kind.id();
            let stack = ItemStack::new(
                name.into(),
                1,
                64,
                native_persistence::nbt::Tag::Compound(Vec::new()),
            )
            .expect("valid sand item");
            self.dropped_items
                .spawn_from_entity(entity.motion.position, stack);
        }
    }

    fn notify_fluid_neighbors(&mut self) {
        if self.notifying_blocks {
            return;
        }
        self.notifying_blocks = true;
        while let Some((position, old, new)) = self.block_notifications.pop_front() {
            self.notify_one_block(position, old, new);
        }
        self.notifying_blocks = false;
    }

    fn notify_one_block(&mut self, position: IVec3, old: BlockStateId, new: BlockStateId) {
        let settings = self.fluid_settings();
        let old_name = self.world.state(old).map(|state| state.name.as_str());
        let new_name = self.world.state(new).map(|state| state.name.as_str());
        let changed_kind = old_name != new_name;
        let new_sand = new_name.and_then(FallingKind::from_id);
        if changed_kind
            && matches!(cell(&self.world, self.origin, self.bounds, position, self.states[0]), Cell::State(id) if id == new)
            && let Some(fluid) = self.ordinary_fluid(new)
            && !(fluid.kind == FluidKind::Lava && self.react_lava(position))
        {
            self.schedule_fluid(position, fluid, settings.delay(fluid.kind));
        }
        if changed_kind
            && matches!(cell(&self.world, self.origin, self.bounds, position, self.states[0]), Cell::State(id) if id == new)
            && let Some(kind) = new_sand
        {
            self.schedule_sand(position, kind);
        }
        for direction in [
            IVec3::NEG_X,
            IVec3::X,
            IVec3::NEG_Y,
            IVec3::Y,
            IVec3::NEG_Z,
            IVec3::Z,
        ] {
            let Some(neighbor) = add(position, direction) else {
                continue;
            };
            let Cell::State(id) = cell(
                &self.world,
                self.origin,
                self.bounds,
                neighbor,
                self.states[0],
            ) else {
                continue;
            };
            let button_support = self
                .world
                .state(id)
                .and_then(stone_button_support_direction);
            if button_support == Some(-direction)
                && matches!(cell(&self.world, self.origin, self.bounds, position, self.states[0]), Cell::State(current) if self.world.state(current).and_then(|state| button_support_face(state, direction)) == Some(false))
                && self.set(neighbor, self.states[0])
            {
                if let Some(absolute) = add(self.origin, neighbor) {
                    self.dropped_items.drop_unattached_block(
                        absolute,
                        "minecraft:stone_button",
                        self.block_drops,
                        self.loot.as_deref(),
                    );
                }
                continue;
            }
            let sand = self
                .world
                .state(id)
                .and_then(|state| FallingKind::from_id(&state.name));
            if let Some(kind) = sand {
                self.schedule_sand(neighbor, kind);
            }
            if let Some(fluid) = self.ordinary_fluid(id)
                && !(fluid.kind == FluidKind::Lava && self.react_lava(neighbor))
            {
                self.schedule_fluid(neighbor, fluid, settings.delay(fluid.kind));
            }
        }
        if let Some(below) = add(position, IVec3::NEG_Y)
            && let Cell::State(id) =
                cell(&self.world, self.origin, self.bounds, below, self.states[0])
            && let Some(kind) = self
                .world
                .state(id)
                .and_then(|state| SoilKind::from_name(&state.name))
            && let Some(absolute) = add(self.origin, below)
        {
            let blocked = self
                .world
                .state(new)
                .and_then(|state| self.soil_cover_state(kind, state));
            if blocked == Some(false) || (blocked.is_none() && kind != SoilKind::Farmland) {
                return;
            }
            let key = (absolute.x, absolute.y, absolute.z, kind);
            if self.soil_pending.insert(key) {
                let sequence = self.block_tick_sequence;
                self.block_tick_sequence = self.block_tick_sequence.wrapping_add(1);
                self.soil_ticks.insert(
                    (
                        i128::from(self.tick) + 1,
                        0,
                        i64::try_from(sequence).unwrap_or(i64::MAX),
                        sequence,
                    ),
                    (absolute, kind),
                );
                self.mark_tick_chunk_dirty(absolute.x, absolute.z);
            }
        }
    }

    fn react_lava(&mut self, position: IVec3) -> bool {
        let Cell::State(id) = cell(
            &self.world,
            self.origin,
            self.bounds,
            position,
            self.states[0],
        ) else {
            return false;
        };
        let Some(lava) = self.ordinary_fluid(id) else {
            return false;
        };
        let reaction = fluid_sim::lava_reaction(
            &|pos| {
                let Cell::State(id) =
                    cell(&self.world, self.origin, self.bounds, pos, self.states[0])
                else {
                    return fluid_sim::LavaNeighbor::Other;
                };
                if self
                    .fluids
                    .get(id.0 as usize)
                    .copied()
                    .flatten()
                    .is_some_and(|fluid| fluid.kind == FluidKind::Water)
                {
                    return fluid_sim::LavaNeighbor::Water;
                }
                match self.world.state(id).map(|state| state.name.as_str()) {
                    Some("minecraft:blue_ice") => fluid_sim::LavaNeighbor::BlueIce,
                    Some("minecraft:soul_soil") => fluid_sim::LavaNeighbor::SoulSoil,
                    _ => fluid_sim::LavaNeighbor::Other,
                }
            },
            position,
            lava,
        );
        let Some(block) = reaction else { return false };
        let id = self.intern_solid(block);
        self.set(position, id)
    }

    fn simulation_cell(&self, position: IVec3) -> fluid_sim::Cell {
        let Cell::State(id) = cell(
            &self.world,
            self.origin,
            self.bounds,
            position,
            self.states[0],
        ) else {
            return fluid_sim::Cell::Unknown;
        };
        let Some(state) = self.world.state(id) else {
            return fluid_sim::Cell::Unknown;
        };
        if matches!(
            state.name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        ) {
            return fluid_sim::Cell::Air;
        }
        if let Some(fluid) = self.ordinary_fluid(id) {
            return fluid_sim::Cell::Fluid(fluid);
        }
        if self.fluids.get(id.0 as usize).copied().flatten().is_some() {
            return fluid_sim::Cell::Unknown;
        }
        let full = self
            .shapes
            .get(id.0 as usize)
            .and_then(Option::as_deref)
            .is_some_and(|boxes| {
                boxes.len() == 1
                    && boxes[0].min == glam::Vec3::ZERO
                    && boxes[0].max == glam::Vec3::ONE
            });
        if full {
            fluid_sim::Cell::Solid
        } else {
            fluid_sim::Cell::Unknown
        }
    }

    fn intern_block(&mut self, block: BlockState) -> BlockStateId {
        let old_count = self.world.state_count();
        let id = Arc::make_mut(&mut self.world).intern_state(block);
        if self.world.state_count() != old_count {
            let state = self.world.state(id).expect("new state is interned");
            let shape = block_shape(state);
            self.unsupported_states += usize::from(shape.is_none());
            self.shapes.push(shape);
            self.outlines.push(block_outline(state));
            self.fluids
                .push(FluidState::from_block(state).ok().flatten());
            if let Some(tags) = &self.heightmap_tags {
                self.heightmap_flags
                    .push(crate::spawn_heightmaps::classify(state, tags).ok());
            }
        }
        id
    }

    fn intern_solid(&mut self, block: fluid_sim::Solidified) -> BlockStateId {
        let block = match block {
            fluid_sim::Solidified::Stone => state("minecraft:stone", &[]),
            fluid_sim::Solidified::Obsidian => state("minecraft:obsidian", &[]),
            fluid_sim::Solidified::Cobblestone => state("minecraft:cobblestone", &[]),
            fluid_sim::Solidified::Basalt => state("minecraft:basalt", &[("axis", "y")]),
        };
        self.intern_block(block)
    }

    fn intern_liquid(&mut self, fluid: FluidState) -> BlockStateId {
        let level = if fluid.source {
            0
        } else {
            8 - fluid.amount + if fluid.falling { 8 } else { 0 }
        };
        let name = match fluid.kind {
            FluidKind::Water => "minecraft:water",
            FluidKind::Lava => "minecraft:lava",
        };
        self.intern_block(state(name, &[("level", &level.to_string())]))
    }

    fn advance_fluid_ticks(&mut self) {
        let settings = self.fluid_settings();
        let mut unloaded = Vec::new();
        let mut executed = 0;
        while let Some((&order, &scheduled)) = self.fluid_ticks.first_key_value() {
            if order.0 > i128::from(self.tick) || executed >= 65_536 {
                break;
            }
            self.fluid_ticks.pop_first();
            let absolute = IVec3::new(scheduled.key.x, scheduled.key.y, scheduled.key.z);
            let relative = IVec3::new(
                match absolute.x.checked_sub(self.origin.x) {
                    Some(x) => x,
                    None => {
                        unloaded.push((order, scheduled));
                        continue;
                    }
                },
                match absolute.y.checked_sub(self.origin.y) {
                    Some(y) => y,
                    None => {
                        unloaded.push((order, scheduled));
                        continue;
                    }
                },
                match absolute.z.checked_sub(self.origin.z) {
                    Some(z) => z,
                    None => {
                        unloaded.push((order, scheduled));
                        continue;
                    }
                },
            );
            let Cell::State(id) = cell(
                &self.world,
                self.origin,
                self.bounds,
                relative,
                self.states[0],
            ) else {
                unloaded.push((order, scheduled));
                continue;
            };
            self.mark_tick_chunk_dirty(absolute.x, absolute.z);
            self.fluid_pending.remove(&scheduled.key);
            executed += 1;
            let Some(fluid) = self.ordinary_fluid(id) else {
                continue;
            };
            if fluid.kind != scheduled.key.kind || fluid.source != scheduled.key.source {
                continue;
            }
            let changes = fluid_sim::tick(
                &|pos| self.simulation_cell(pos),
                relative,
                fluid.kind,
                settings,
            );
            for change in changes {
                let id = match change.block {
                    fluid_sim::ChangedBlock::Air => self.states[0],
                    fluid_sim::ChangedBlock::Fluid(fluid) => self.intern_liquid(fluid),
                    fluid_sim::ChangedBlock::Solid(block) => self.intern_solid(block),
                };
                if self.set(change.position, id)
                    && change.position == relative
                    && let fluid_sim::ChangedBlock::Fluid(next) = change.block
                {
                    // A changed flowing state schedules itself again. Lava's
                    // random fourfold delay is not implemented yet.
                    self.schedule_fluid(relative, next, settings.delay(next.kind));
                }
            }
        }
        self.fluid_ticks.extend(unloaded);
    }

    fn advance_tick(&mut self) {
        // MinecraftServer advances shared clocks before ticking its levels.
        for clock in self.world_clocks.values_mut() {
            clock.tick(self.advance_time);
        }
        if let Some(environment) = &self.fast_lava_environment
            && let Some(value) = environment.sample(|id| {
                self.world_clocks
                    .iter()
                    .find(|(clock, _)| clock.namespace == id.namespace && clock.path == id.path)
                    .map(|(_, state)| state.total_ticks)
            })
        {
            self.fast_lava = value;
        }
        // Java ticks the camera before ticking the player entity.
        let camera_target_eye_height = self.player.pose.eye_height();
        let camera_sprinting = self.sprint.active;
        let camera_flying = self.flight.flying;
        self.refresh_movement_speed();
        let camera_speed = self.movement_speed(camera_sprinting);
        self.apply_streaming();
        // ServerLevel runs its scheduled block ticks before its fluid ticks.
        self.advance_block_ticks();
        self.advance_fluid_ticks();
        self.advance_falling_sand();
        if self.death.is_some() {
            let requested = self
                .commands
                .drain(..)
                .any(|command| matches!(command, PlayerCommand::Respawn));
            let eligible = self.death.is_some_and(|death| {
                !self.vitals.hardcore && !death.inventory_unresolved && self.respawn_death_safe
            });
            let has_respawn_path = self.respawn_support.is_some()
                || (self.saved_respawn_absent && self.world_respawn_support.is_some());
            if eligible
                && ((requested && self.death.is_some_and(|death| death.can_respawn))
                    || (self.death_rules.immediate_respawn && has_respawn_path))
            {
                self.begin_respawn_request();
            }
            self.refresh_respawn_availability();
            if self.respawn_request_tick.is_some() && self.respawn_target_ready {
                match self.respawn_at_saved_position() {
                    Ok(true) => {
                        self.tick += 1;
                        self.mark_advanced_saved_tick_chunks_dirty();
                        return;
                    }
                    Ok(false) => {}
                    Err(error) => self
                        .stream_errors
                        .push(format!("cannot respawn: {error:#}")),
                }
            }
            self.mining.release();
            self.food_use.cancel();
            self.camera.begin_bob_tick(DVec3::ZERO, false);
            self.camera.tick_with_speed(
                self.motion.position,
                self.player.pose.eye_height(),
                camera_speed,
                self.walking_speed,
                camera_flying,
                self.fov_effect_scale,
            );
            let world = &self.world;
            let shapes = &self.shapes;
            let origin = self.origin;
            let bounds = self.bounds;
            let air = self.states[0];
            let bounce_suppression = &self.bounce_suppression_tag;
            let fluid_view = FluidView {
                world,
                shapes,
                fluids: &self.fluids,
                origin,
                bounds,
                air,
            };
            self.dropped_items.tick_with_fluid(
                origin,
                self.motion.position,
                self.player.pose,
                GameMode::Spectator,
                &mut self.inventory,
                |pos| match cell(world, origin, bounds, pos, air) {
                    Cell::Missing => unknown_barrier(),
                    Cell::State(id) => shapes
                        .get(id.0 as usize)
                        .and_then(Option::as_deref)
                        .unwrap_or_else(|| unknown_barrier()),
                },
                |pos| match cell(world, origin, bounds, pos, air) {
                    Cell::State(id) => world
                        .state(id)
                        .map_or(0.6, |state| crate::physics::ground_friction(&state.name)),
                    Cell::Missing => 0.6,
                },
                |pos| match cell(world, origin, bounds, pos, air) {
                    Cell::State(id) => world.state(id).map_or(0.0, |state| {
                        bounce_suppression
                            .as_ref()
                            .filter(|tag| !tag.contains(&state.name))
                            .map_or(0.0, |_| crate::items::block_bounce_restitution(&state.name))
                    }),
                    Cell::Missing => 0.0,
                },
                |pos| match cell(world, origin, bounds, pos, air) {
                    Cell::State(id) => world.state(id).is_some_and(|state| {
                        state.name.ends_with("_wall") || state.name.ends_with("_fence_gate")
                    }),
                    Cell::Missing => false,
                },
                |item_bounds| {
                    if fluid_view.chunks_loaded(item_bounds) {
                        fluid::contacts(
                            item_bounds,
                            item_bounds.min.y,
                            &|pos| fluid_view.at(pos),
                            &|pos, direction| fluid_view.sturdy_face(pos, direction),
                        )
                    } else {
                        Default::default()
                    }
                },
                self.fast_lava,
                |bounds, actual, requested| {
                    fluid::swept_effects(bounds, actual, requested, &|pos| fluid_view.at(pos))
                },
            );
            self.tick_experience_orbs(false);
            self.tick += 1;
            self.mark_advanced_saved_tick_chunks_dirty();
            return;
        }
        self.use_repeat.advance_tick();
        let mut repeated_use = false;
        loop {
            let command = if let Some(command) = self.commands.pop_front() {
                command
            } else if !repeated_use
                && self.use_repeat.ready()
                && !self.food_use.is_active()
                && !self.inventory.is_open()
            {
                repeated_use = true;
                PlayerCommand::Place
            } else {
                break;
            };
            let before_inventory = Arc::clone(&self.inventory);
            match command {
                PlayerCommand::Respawn => {}
                PlayerCommand::Move {
                    forward,
                    strafe,
                    jump,
                } => {
                    self.movement = (
                        if forward.is_finite() {
                            forward.clamp(-1.0, 1.0)
                        } else {
                            0.0
                        },
                        if strafe.is_finite() {
                            strafe.clamp(-1.0, 1.0)
                        } else {
                            0.0
                        },
                        jump,
                    )
                }
                PlayerCommand::Sprint {
                    pressed,
                    backward_pressed,
                } => {
                    self.sprint.pressed = pressed;
                    self.sprint.backward_pressed = backward_pressed;
                }
                PlayerCommand::Sneak(pressed) => self.sneak = pressed,
                PlayerCommand::Look { yaw, pitch } if yaw.is_finite() && pitch.is_finite() => {
                    self.player.yaw += yaw;
                    self.player.pitch =
                        (self.player.pitch + pitch).clamp(-crate::MAX_PITCH, crate::MAX_PITCH);
                }
                PlayerCommand::Look { .. } => {}
                PlayerCommand::Select(block) => select_block(&mut self.inventory, block),
                PlayerCommand::SelectSlot(slot) => {
                    Arc::make_mut(&mut self.inventory).select(slot);
                }
                PlayerCommand::AdjustFlightSpeed(steps) => {
                    if self.game_mode == GameMode::Spectator {
                        self.flight_speed =
                            crate::physics::adjust_flight_speed(self.flight_speed, steps);
                    }
                }
                PlayerCommand::ToggleInventory => {
                    self.mining.release();
                    self.food_use.cancel();
                    if let Some(drops) = Arc::make_mut(&mut self.inventory).toggle_with_drops() {
                        for stack in drops {
                            self.dropped_items.drop_player(
                                stack,
                                self.origin,
                                self.motion.position,
                                self.player,
                                crate::items::PlayerDropOrigin::ContainerReturn,
                            );
                        }
                    }
                }
                PlayerCommand::InventoryClick { slot, button }
                    if self.game_mode != GameMode::Spectator =>
                {
                    inventory_click(
                        &mut self.inventory,
                        self.recipes.as_deref(),
                        slot,
                        button,
                        self.game_mode == GameMode::Creative,
                    );
                }
                PlayerCommand::InventoryQuickMove { slot }
                    if self.game_mode != GameMode::Spectator =>
                {
                    for stack in inventory_quick_move(
                        &mut self.inventory,
                        self.recipes.as_deref(),
                        slot,
                        self.game_mode == GameMode::Creative,
                    ) {
                        self.dropped_items.drop_player(
                            stack,
                            self.origin,
                            self.motion.position,
                            self.player,
                            crate::items::PlayerDropOrigin::ContainerReturn,
                        );
                    }
                }
                PlayerCommand::InventoryPointer(event) if self.game_mode != GameMode::Spectator => {
                    for (stack, drop_origin) in inventory_pointer(
                        &mut self.inventory,
                        self.recipes.as_deref(),
                        event,
                        self.game_mode == GameMode::Creative,
                    ) {
                        self.dropped_items.drop_player(
                            stack,
                            self.origin,
                            self.motion.position,
                            self.player,
                            drop_origin,
                        );
                    }
                }
                PlayerCommand::InventoryPointer(_) => {}
                PlayerCommand::InventoryClick { .. } | PlayerCommand::InventoryQuickMove { .. } => {
                }
                PlayerCommand::RecipeBookSettings { open, filtering } => {
                    if self.inventory.is_open() {
                        Arc::make_mut(&mut self.inventory)
                            .set_recipe_book_settings(open, filtering);
                    }
                }
                PlayerCommand::RecipeBookPlace { recipe, use_max } => {
                    if self.game_mode != GameMode::Spectator
                        && let Some(recipes) = &self.recipes
                        && let Some(recipe) = recipes.at(recipe)
                    {
                        Arc::make_mut(&mut self.inventory)
                            .place_book_recipe(recipe, recipes, use_max);
                    }
                }
                PlayerCommand::SwapHands if self.game_mode != GameMode::Spectator => {
                    Arc::make_mut(&mut self.inventory).swap_hands();
                }
                PlayerCommand::InventorySwap { slot, target }
                    if self.game_mode != GameMode::Spectator =>
                {
                    if let Some(drops) = Arc::make_mut(&mut self.inventory).swap_slot(
                        slot,
                        target,
                        self.game_mode == GameMode::Creative,
                        self.recipes.as_deref(),
                    ) {
                        for stack in drops {
                            self.dropped_items.drop_player(
                                stack,
                                self.origin,
                                self.motion.position,
                                self.player,
                                crate::items::PlayerDropOrigin::Direct,
                            );
                        }
                    }
                }
                PlayerCommand::SwapHands | PlayerCommand::InventorySwap { .. } => {}
                PlayerCommand::DropSelected { all } if self.game_mode != GameMode::Spectator => {
                    if let Some(stack) = Arc::make_mut(&mut self.inventory).drop_selected(all) {
                        self.mining.release();
                        self.dropped_items.drop_player(
                            stack,
                            self.origin,
                            self.motion.position,
                            self.player,
                            crate::items::PlayerDropOrigin::Direct,
                        );
                    }
                }
                PlayerCommand::InventoryThrow { slot, all }
                    if self.game_mode != GameMode::Spectator =>
                {
                    if let Some(stack) = inventory_throw(
                        &mut self.inventory,
                        self.recipes.as_deref(),
                        slot,
                        all,
                        self.game_mode == GameMode::Creative,
                    ) {
                        self.dropped_items.drop_player(
                            stack,
                            self.origin,
                            self.motion.position,
                            self.player,
                            crate::items::PlayerDropOrigin::Direct,
                        );
                    }
                }
                PlayerCommand::DropCursor { button } if self.game_mode != GameMode::Spectator => {
                    if let Some(stack) = Arc::make_mut(&mut self.inventory).drop_cursor(button) {
                        self.dropped_items.drop_player(
                            stack,
                            self.origin,
                            self.motion.position,
                            self.player,
                            crate::items::PlayerDropOrigin::Direct,
                        );
                    }
                }
                PlayerCommand::DropSelected { .. }
                | PlayerCommand::InventoryThrow { .. }
                | PlayerCommand::DropCursor { .. } => {}
                PlayerCommand::PickBlock
                    if self.game_mode != GameMode::Spectator && !self.inventory.is_open() =>
                {
                    let reach = if self.game_mode == GameMode::Creative {
                        5.0
                    } else {
                        4.5
                    };
                    if let Some(hit) = self.ray_hit()
                        && hit.location.distance_squared(self.player.eye().as_dvec3())
                            <= reach * reach
                        && let Cell::State(id) = cell(
                            &self.world,
                            self.origin,
                            self.bounds,
                            hit.position,
                            self.states[0],
                        )
                        && let Some(block) = self
                            .world
                            .state(id)
                            .and_then(|state| Block::from_item_id(&state.name))
                        && let Some(item) = crate::picked_block_item(block)
                    {
                        Arc::make_mut(&mut self.inventory)
                            .pick_item(item, self.game_mode == GameMode::Creative);
                    }
                }
                PlayerCommand::PickBlock => {}
                PlayerCommand::Break if self.food_use.is_active() && !self.inventory.is_open() => {
                    self.mining.hold_without_start();
                }
                PlayerCommand::Break
                    if self.game_mode.allows_basic_building() && !self.inventory.is_open() =>
                {
                    self.first_person.swing.start(crate::animation::Hand::Main);
                    let target = self.mining_target();
                    let context = self.mining_context();
                    let hand = self.inventory.slot(crate::inventory::PlayerSlot::Storage(
                        self.inventory.selected(),
                    ));
                    if let Some(position) = self.mining.press_with_context(
                        target,
                        hand,
                        self.game_mode == GameMode::Creative,
                        context,
                    ) {
                        self.finish_breaking(position);
                    }
                }
                PlayerCommand::Break
                    if self.game_mode != GameMode::Spectator && !self.inventory.is_open() =>
                {
                    self.first_person.swing.start(crate::animation::Hand::Main);
                }
                PlayerCommand::Break => {}
                PlayerCommand::StopBreak => self.mining.release(),
                PlayerCommand::StopUse => {
                    self.use_repeat.release();
                    self.food_use.cancel();
                }
                PlayerCommand::UseHeld(held) => {
                    self.use_repeat.set_held(held);
                    self.food_use.set_held(held);
                }
                PlayerCommand::Place
                    if self.food_use.is_active() || self.mining.is_destroying() => {}
                PlayerCommand::Place if !self.inventory.is_open() => {
                    self.use_repeat.used();
                    let main_hand =
                        crate::inventory::PlayerSlot::Storage(self.inventory.selected());
                    let mut handled = false;
                    if let Some(hit) = self.ray_hit() {
                        let main = self.inventory.slot(crate::inventory::PlayerSlot::Storage(
                            self.inventory.selected(),
                        ));
                        let off = self.inventory.slot(crate::inventory::PlayerSlot::Offhand);
                        let bypass = self.sneak && (main.is_some() || off.is_some());
                        let table = matches!(cell(&self.world, self.origin, self.bounds, hit.position, self.states[0]), Cell::State(id) if self.world.state(id).is_some_and(|state| state.name == "minecraft:crafting_table"));
                        if self.game_mode != GameMode::Spectator
                            && !bypass
                            && self.use_stone_button(hit.position)
                        {
                            handled = true;
                        } else if table && (self.game_mode == GameMode::Spectator || !bypass) {
                            handled = true;
                            self.mining.release();
                            if let Some(position) = add(self.origin, hit.position)
                                && Arc::make_mut(&mut self.inventory).open_crafting_table(position)
                                && self.game_mode != GameMode::Spectator
                            {
                                self.first_person.swing.start(crate::animation::Hand::Main);
                            }
                        } else if self.food_use.start_hand(
                            &self.inventory,
                            main_hand,
                            self.vitals,
                            self.game_mode,
                        ) || (self.game_mode.allows_basic_building()
                            && self.use_tool(hit.position, hit.face))
                        {
                            handled = true;
                        } else if self.game_mode.allows_basic_building()
                            && let Some(hand) = self.place_stone_button(hit)
                        {
                            handled = true;
                            let swing = if hand == crate::inventory::PlayerSlot::Offhand {
                                crate::animation::Hand::Off
                            } else {
                                crate::animation::Hand::Main
                            };
                            self.first_person.swing.start(swing);
                            if self.game_mode.consumes_placed_item() {
                                Arc::make_mut(&mut self.inventory).consume_held(hand);
                            }
                            if self.inventory.slot(hand).is_some() {
                                self.first_person.used(swing);
                            }
                        } else if self.game_mode.allows_basic_building()
                            && let Some((hand, block)) = placement_hand(&self.inventory)
                            && let Some(target) = add(hit.position, hit.face)
                            && matches!(cell(&self.world, self.origin, self.bounds, target, self.states[0]), Cell::State(id) if self.world.state(id).is_some_and(|state| matches!(state.name.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")))
                            && let Some(block) = self.placement_block(target, block)
                            && !player_intersects_block(self.player, target, block)
                            && {
                                let id = self.placement_id(block.orient_for_face(hit.face));
                                self.set(target, id)
                            }
                        {
                            handled = true;
                            let swing_hand = if hand == crate::inventory::PlayerSlot::Offhand {
                                crate::animation::Hand::Off
                            } else {
                                crate::animation::Hand::Main
                            };
                            self.first_person.swing.start(swing_hand);
                            if self.game_mode.consumes_placed_item() {
                                Arc::make_mut(&mut self.inventory).consume_held(hand);
                            }
                            if self.inventory.slot(hand).is_some() {
                                self.first_person.used(swing_hand);
                            }
                        }
                    }
                    if !handled
                        && !self.food_use.start_hand(
                            &self.inventory,
                            main_hand,
                            self.vitals,
                            self.game_mode,
                        )
                    {
                        self.food_use.start_hand(
                            &self.inventory,
                            crate::inventory::PlayerSlot::Offhand,
                            self.vitals,
                            self.game_mode,
                        );
                    }
                }
                PlayerCommand::Place => {}
            }
            update_recipe_unlocks(
                &mut self.inventory,
                &before_inventory,
                self.recipes.as_deref(),
            );
            self.selected = selected_inventory_block(&self.inventory);
        }
        if self.mining.held()
            && !self.food_use.is_active()
            && self.game_mode.allows_basic_building()
        {
            let target = self.mining_target();
            if target.is_some() {
                self.first_person.swing.start(crate::animation::Hand::Main);
            }
            let context = self.mining_context();
            let hand = self.inventory.slot(crate::inventory::PlayerSlot::Storage(
                self.inventory.selected(),
            ));
            if let Some(position) = self.mining.tick_with_context(
                target,
                hand,
                self.game_mode == GameMode::Creative,
                context,
            ) {
                self.finish_breaking(position);
            }
        }
        if let Err(error) = self.armor.refresh(&self.inventory) {
            self.stream_errors
                .push(format!("player armor equipment: {error:#}"));
        }
        self.food_use
            .prepare(&self.inventory, self.vitals, self.game_mode);
        // baseTick reads the previous equipment attributes before detecting changes.
        let breathing_oxygen = self.oxygen_bonus;
        let breathing_oxygen_valid = self.oxygen_valid;
        self.refresh_equipment_oxygen();
        self.refresh_bounciness();
        self.refresh_jump_strength();
        self.refresh_gravity();
        self.refresh_water_movement_efficiency();
        self.refresh_mining_attributes();
        let friction = self.friction_below();
        let fluid_view = FluidView {
            world: &self.world,
            shapes: &self.shapes,
            fluids: &self.fluids,
            origin: self.origin,
            bounds: self.bounds,
            air: self.states[0],
        };
        let player_bounds =
            crate::movement::player_bounds(self.motion.position, self.player.height());
        let fluid_chunks_loaded = fluid_view.chunks_loaded(player_bounds);
        let contacts = if fluid_chunks_loaded {
            fluid::contacts(
                player_bounds,
                self.motion.position.y + f64::from(self.player.pose.eye_height()),
                &|pos| fluid_view.at(pos),
                &|pos, direction| fluid_view.sturdy_face(pos, direction),
            )
        } else {
            fluid::FluidContacts::default()
        };
        let in_water = contacts.water.depth > 0.0;
        let under_water = in_water && self.previous_eye_in_water;
        self.previous_eye_in_water = contacts.water.eyes_inside;
        if fluid_chunks_loaded && breathing_oxygen_valid {
            let bubble_column = contacts.water.eyes_inside
                && {
                    let eye_position = (self.motion.position
                        + DVec3::Y * f64::from(self.player.pose.eye_height()))
                    .floor()
                    .as_ivec3();
                    matches!(
                        cell(&self.world, self.origin, self.bounds, eye_position, self.states[0]),
                        Cell::State(id) if self.world.state(id).is_some_and(|state| state.name == "minecraft:bubble_column")
                    )
                };
            self.vitals.tick_air_with(
                contacts.water.eyes_inside,
                bubble_column,
                self.game_mode,
                &self.breathing_effects,
                breathing_oxygen,
                &mut || self.player_random.next_double(),
            );
            if self.vitals.health <= 0.0 {
                self.death = Some(DeathInfo {
                    cause: DeathCause::Drown,
                    tick: self.tick.saturating_add(1),
                    score: self.score,
                    show_message: self.death_rules.show_death_messages,
                    inventory_unresolved: false,
                    respawn_pending: false,
                    can_respawn: false,
                });
                self.movement = (0.0, 0.0, false);
                self.sprint = crate::physics::SprintState::default();
            }
        }
        let tracking = self
            .effect_save_tracking
            .get_or_insert_with(|| EffectSaveTracking {
                breathing: self.breathing_effects.clone(),
                fall: self.fall.effects().clone(),
                movement_speed: self.movement_speed_attribute.clone(),
                gravity: self.gravity_attribute.clone(),
                water_efficiency: self.water_efficiency_attribute.clone(),
                mining_attributes: self.mining_attributes.clone(),
                ticks: 0,
            });
        tracking.ticks = tracking.ticks.saturating_add(1);
        self.breathing_effects.tick();
        let transitions =
            [&self.fall.effects().speed, &self.fall.effects().slowness].map(|effect| {
                effect
                    .as_ref()
                    .is_some_and(|effect| effect.duration != -1 && effect.duration <= 1)
            });
        self.fall.tick_effects();
        self.update_movement_effect_modifiers(transitions);
        self.refresh_movement_speed();
        let fluid_view = FluidView {
            world: &self.world,
            shapes: &self.shapes,
            fluids: &self.fluids,
            origin: self.origin,
            bounds: self.bounds,
            air: self.states[0],
        };
        let block_shape = |cell_pos| match cell(
            &self.world,
            self.origin,
            self.bounds,
            cell_pos,
            self.states[0],
        ) {
            Cell::Missing => unknown_barrier(),
            Cell::State(id) => self
                .shapes
                .get(id.0 as usize)
                .and_then(Option::as_deref)
                .unwrap_or_else(|| unknown_barrier()),
        };
        self.swimming = if self.flight.flying {
            false
        } else if self.swimming {
            self.sprint.active && in_water
        } else {
            self.sprint.active
                && under_water
                && matches!(fluid_view.at(self.motion.position.floor().as_ivec3()), fluid::Cell::Fluid(state) if state.kind == FluidKind::Water)
        };
        if !self.flight.flying {
            self.motion.velocity = contacts.water.push(self.motion.velocity, 0.014);
            let lava_strength = if self.world.dimension == "minecraft:the_nether" {
                0.007
            } else {
                0.0023333333333333335
            };
            self.motion.velocity = contacts.lava.push(self.motion.velocity, lava_strength);
        }
        if in_water {
            self.fall_distance = 0.0;
        } else if contacts.lava.depth > 0.0 {
            self.fall_distance *= 0.5;
        }
        let fits_crouching =
            crate::movement::can_fit(self.motion.position, CROUCH_HEIGHT, block_shape);
        let fits_standing =
            crate::movement::can_fit(self.motion.position, PLAYER_HEIGHT, block_shape);
        let moving_slowly = !self.flight.flying
            && ((!self.swimming && fits_crouching && (self.previous_sneak || !fits_standing))
                || (self.player.pose == PlayerPose::Crawling && !in_water));
        self.sprint.previous_sneak = self.previous_sneak;
        self.sprint.moving_slowly = moving_slowly;
        self.sprint.insufficient_food = !self.vitals.can_sprint(self.game_mode);
        self.sprint.using_item = self.food_use.is_active();
        self.sprint.in_water = in_water;
        self.sprint.under_water = under_water;
        self.sprint.swimming = self.swimming;
        self.sprint.on_ground = self.motion.on_ground;
        self.sprint.current_sneak = self.sneak;
        self.camera.begin_bob_tick(
            self.motion.velocity,
            self.motion.on_ground && self.vitals.health > 0.0 && !self.swimming,
        );
        let input = crate::physics::MovementInput {
            forward: self.movement.0,
            right: self.movement.1,
            yaw_degrees: self.player.yaw.to_degrees() - 180.0,
            jump: self.movement.2,
            sprinting: self
                .sprint
                .update_with_window(self.movement.0, self.sprint_window),
            using_item: self.food_use.is_active(),
            moving_slowly,
        };
        let jump_power = crate::physics::jump_power(
            self.jump_strength.unwrap_or(f64::from(0.42_f32)),
            self.block_jump_factor(),
            self.fall
                .effects()
                .jump_boost
                .as_ref()
                .map(|effect| effect.amplifier),
        );
        let slow_falling = self.fall.effects().slow_falling.is_some();
        let gravity = if self.no_gravity {
            0.0
        } else {
            self.gravity.unwrap_or(0.08)
        };
        let movement_speed = self.movement_speed(input.sprinting);
        let levitation = self
            .fall
            .effects()
            .levitation
            .as_ref()
            .map(|effect| effect.amplifier);
        if slow_falling || levitation.is_some() {
            self.fall_distance = 0.0;
        }
        let start_flight_jump = self.flight.update(
            self.game_mode,
            input.jump,
            self.swimming,
            self.motion.on_ground,
        );
        if start_flight_jump {
            self.motion.start_flying_jump_with_power(input, jump_power);
        }
        let flying = self.flight.flying;
        let player_height = self.player.height();
        let fall_distance = self.fall_distance;
        let sneak = self.sneak;
        let resolve = |position, requested, on_ground| {
            let adjusted = crate::movement::back_off(
                position,
                requested,
                on_ground,
                fall_distance,
                player_height,
                sneak,
                block_shape,
            );
            let actual = crate::movement::resolve_player(
                position,
                adjusted,
                on_ground,
                player_height,
                block_shape,
            );
            (adjusted, actual)
        };
        let liquid = if in_water {
            Some((FluidKind::Water, contacts.water.depth))
        } else if contacts.lava.depth > 0.0 {
            Some((FluidKind::Lava, contacts.lava.depth))
        } else {
            None
        };
        if self.death.is_none() {
            self.vitals.tick_peaceful_regeneration(
                self.survival_rules,
                (self.tick as i32).wrapping_add(1),
            );
        }
        let jumps_from_ground = input.jump
            && self.motion.on_ground
            && self.motion.jump_delay <= 1
            && liquid.is_none_or(|(_, depth)| depth <= 0.4);
        if jumps_from_ground {
            self.vitals.record_jump(self.game_mode, input.sprinting);
        }
        let before_move = self.motion.position;
        let collision_support = std::cell::Cell::new(None);
        let world = &self.world;
        let origin = self.origin;
        let bounds = self.bounds;
        let air = self.states[0];
        let no_previous_support = self.ground_without_support;
        let suppressed = &self.bounce_suppression_tag;
        let entity_bounce = if sneak {
            0.0
        } else {
            self.bounciness.unwrap_or(0.0)
        };
        let restitution = |position: DVec3, requested: DVec3, actual: DVec3| {
            let collision_gravity = if slow_falling && requested.y <= 0.0 {
                gravity.min(0.01)
            } else {
                gravity
            };
            if requested.y == actual.y {
                return (entity_bounce, 0.0);
            }
            if requested.y >= 0.0 {
                return (
                    entity_bounce,
                    crate::items::collision_restitution(
                        requested.y,
                        actual.y,
                        entity_bounce,
                        collision_gravity,
                    ),
                );
            }
            if sneak {
                return (0.0, 0.0);
            }
            let support = crate::movement::supporting_block(
                position,
                actual,
                no_previous_support,
                block_shape,
            );
            collision_support.set(Some(support));
            let below = crate::movement::movement_on_block(position, support, |pos| {
                match cell(world, origin, bounds, pos, air) {
                    Cell::State(id) => world.state(id).is_some_and(|state| {
                        state.name.ends_with("_wall") || state.name.ends_with("_fence_gate")
                    }),
                    Cell::Missing => false,
                }
            });
            let bounce = match cell(world, origin, bounds, below, air) {
                Cell::State(id) => world
                    .state(id)
                    .and_then(|state| {
                        suppressed
                            .as_ref()
                            .filter(|tag| !tag.contains(&state.name))
                            .map(|_| {
                                entity_bounce.max(f64::from(
                                    crate::items::block_bounce_restitution(&state.name),
                                ))
                            })
                    })
                    .unwrap_or(0.0),
                Cell::Missing => 0.0,
            };
            (
                entity_bounce,
                crate::items::vertical_restitution(
                    requested.y,
                    actual.y,
                    bounce,
                    collision_gravity,
                ),
            )
        };
        let (major_collision, actual) = if self.game_mode == GameMode::Spectator {
            (
                false,
                self.motion
                    .tick_spectator(input, self.sneak, self.flight_speed),
            )
        } else if flying {
            self.motion.tick_flying_with_movement_speed(
                input,
                self.sneak,
                self.flight_speed,
                friction,
                entity_bounce,
                movement_speed,
                |position, requested, on_ground| {
                    (
                        requested,
                        crate::movement::resolve_player(
                            position,
                            requested,
                            on_ground,
                            player_height,
                            block_shape,
                        ),
                    )
                },
            )
        } else if let Some((kind, depth)) = liquid {
            let above = (self.motion.position + DVec3::Y * 0.9).floor().as_ivec3();
            let movement = crate::physics::FluidMovement {
                kind,
                depth,
                descend: self.sneak,
                swimming: self.swimming,
                look_y: crate::physics::look_vertical(self.player.pitch),
                fluid_above: matches!(fluid_view.at(above), fluid::Cell::Fluid(_)),
            };
            self.motion.tick_fluid_with_restitution(
                input,
                movement,
                crate::physics::WaterMovement {
                    efficiency: self.water_efficiency.unwrap_or(0.0) as f32,
                    movement_speed,
                    dolphins_grace: self.fall.effects().dolphins_grace.is_some(),
                },
                jump_power,
                gravity,
                slow_falling,
                resolve,
                |position, offset| {
                    let target = position + offset;
                    crate::movement::can_fit_exact(target, player_height, block_shape)
                        && !fluid_view
                            .contains_liquid(crate::movement::player_bounds(target, player_height))
                },
                restitution,
            )
        } else {
            self.motion.tick_resolved_with_restitution(
                input,
                friction,
                movement_speed,
                jump_power,
                gravity,
                slow_falling,
                levitation,
                resolve,
                restitution,
            )
        };
        self.camera
            .finish_bob_tick(self.motion.position - before_move);
        self.vitals.record_movement(
            self.game_mode,
            actual,
            crate::survival::MovementEnvironment {
                on_ground: self.motion.on_ground,
                sprinting: input.sprinting,
                swimming: self.swimming,
                eyes_in_water: contacts.water.eyes_inside,
                in_water,
            },
        );
        self.sprint.after_move(major_collision);
        self.flight
            .after_movement(self.game_mode, self.motion.on_ground);
        // LivingEntity refreshes fluid interaction after moving if it was not
        // already in water, so entering water on the landing tick cancels fall damage.
        let moved_bounds = crate::movement::player_bounds(self.motion.position, player_height);
        let landing_in_water = in_water
            || (actual != DVec3::ZERO
                && fluid_view.chunks_loaded(moved_bounds)
                && fluid::contacts(
                    moved_bounds,
                    self.motion.position.y + f64::from(self.player.pose.eye_height()),
                    &|pos| fluid_view.at(pos),
                    &|pos, direction| fluid_view.sturdy_face(pos, direction),
                )
                .water
                .depth
                    > 0.0);
        if flying || landing_in_water {
            self.fall_distance = 0.0;
        } else {
            self.fall_distance -= f64::from(actual.y.min(0.0) as f32);
        }
        if self.motion.on_ground {
            let support = collision_support.get().unwrap_or_else(|| {
                crate::movement::supporting_block(
                    self.motion.position,
                    actual,
                    self.ground_without_support,
                    block_shape,
                )
            });
            self.ground_without_support = support.is_none();
            self.ground_support = support.and_then(|relative| add(self.origin, relative));
            let legacy = crate::movement::legacy_on_block(self.motion.position, support, |pos| {
                match cell(&self.world, self.origin, self.bounds, pos, self.states[0]) {
                    Cell::State(id) => self.world.state(id).is_some_and(|state| {
                        let name = state.name.as_str();
                        name.ends_with("_fence")
                            || name.ends_with("_wall")
                            || name.ends_with("_fence_gate")
                    }),
                    Cell::Missing => false,
                }
            });
            let landing_state = match cell(
                &self.world,
                self.origin,
                self.bounds,
                legacy,
                self.states[0],
            ) {
                Cell::State(id) if self.shapes.get(id.0 as usize).is_some_and(Option::is_some) => {
                    Some(id)
                }
                _ => None,
            };
            if self.game_mode != GameMode::Spectator
                && self.fall_distance > 0.0
                && landing_state
                    .and_then(|id| self.world.state(id))
                    .is_some_and(|state| state.name == "minecraft:farmland")
                && crate::farmland_trampled_by_player(
                    &mut self.level_random,
                    self.fall_distance,
                    player_height,
                )
                && let Some(absolute) = add(self.origin, legacy)
            {
                self.push_entities_from_soil(legacy, absolute);
                self.set(legacy, self.states[2]);
            }
            let state = landing_state.and_then(|id| self.world.state(id));
            if let Some(state) = state
                && let Err(error) = self.fall.land(
                    self.fall_distance,
                    Some(state),
                    &self.inventory,
                    self.armor.value(),
                    &mut self.vitals,
                    self.game_mode,
                    self.survival_rules,
                )
            {
                self.stream_errors
                    .push(format!("player fall damage: {error:#}"));
            }
            if self.death.is_none() && self.vitals.health <= 0.0 {
                self.death = Some(DeathInfo {
                    cause: if state.is_some_and(|state| state.name == "minecraft:pointed_dripstone")
                    {
                        DeathCause::Stalagmite
                    } else if self.fall_distance > 5.0 {
                        DeathCause::FallHigh
                    } else {
                        DeathCause::Fall
                    },
                    tick: self.tick.saturating_add(1),
                    score: self.score,
                    show_message: self.death_rules.show_death_messages,
                    inventory_unresolved: false,
                    respawn_pending: false,
                    can_respawn: false,
                });
            }
            self.fall_distance = 0.0;
        } else {
            self.ground_without_support = false;
            self.ground_support = None;
        }
        let block_shape = |cell_pos| match cell(
            &self.world,
            self.origin,
            self.bounds,
            cell_pos,
            self.states[0],
        ) {
            Cell::Missing => unknown_barrier(),
            Cell::State(id) => self
                .shapes
                .get(id.0 as usize)
                .and_then(Option::as_deref)
                .unwrap_or_else(|| unknown_barrier()),
        };
        let fits_standing =
            crate::movement::can_fit(self.motion.position, PLAYER_HEIGHT, block_shape);
        let fits_crouching =
            crate::movement::can_fit(self.motion.position, CROUCH_HEIGHT, block_shape);
        let fits_crawling =
            crate::movement::can_fit(self.motion.position, CRAWL_HEIGHT, block_shape);
        self.player.pose = if self.game_mode == GameMode::Spectator {
            PlayerPose::Standing
        } else {
            self.player.pose.after_movement(
                self.sneak && !self.flight.flying,
                self.swimming,
                fits_standing,
                fits_crouching,
                fits_crawling,
            )
        };
        self.previous_sneak = self.sneak;
        self.player.position = self.motion.position.as_vec3();
        self.player.vertical_velocity = (self.motion.velocity.y * 20.0) as f32;
        self.player.on_ground = self.motion.on_ground;
        self.camera.tick_with_speed(
            self.motion.position,
            camera_target_eye_height,
            camera_speed,
            self.walking_speed,
            camera_flying,
            self.fov_effect_scale,
        );
        if let Some(InventoryMenu::CraftingTable { position }) = self.inventory.menu() {
            let relative = subtract(position, self.origin);
            let valid = relative.is_some_and(|relative| {
                matches!(cell(&self.world, self.origin, self.bounds, relative, self.states[0]), Cell::State(id) if self.world.state(id).is_some_and(|state| state.name == "minecraft:crafting_table"))
                    && within_crafting_range(self.motion.position, self.player.pose, relative, self.game_mode)
            });
            if !valid && let Some(drops) = Arc::make_mut(&mut self.inventory).toggle_with_drops() {
                for stack in drops {
                    self.dropped_items.drop_player(
                        stack,
                        self.origin,
                        self.motion.position,
                        self.player,
                        crate::items::PlayerDropOrigin::ContainerReturn,
                    );
                }
            }
        }
        let world = &self.world;
        let shapes = &self.shapes;
        let origin = self.origin;
        let bounds = self.bounds;
        let air = self.states[0];
        let bounce_suppression = &self.bounce_suppression_tag;
        let before_pickup = Arc::clone(&self.inventory);
        let fluid_view = FluidView {
            world,
            shapes,
            fluids: &self.fluids,
            origin,
            bounds,
            air,
        };
        self.dropped_items.tick_with_fluid(
            origin,
            self.motion.position,
            self.player.pose,
            if self.death.is_some() {
                GameMode::Spectator
            } else {
                self.game_mode
            },
            &mut self.inventory,
            |pos| match cell(world, origin, bounds, pos, air) {
                Cell::Missing => unknown_barrier(),
                Cell::State(id) => shapes
                    .get(id.0 as usize)
                    .and_then(Option::as_deref)
                    .unwrap_or_else(|| unknown_barrier()),
            },
            |pos| match cell(world, origin, bounds, pos, air) {
                Cell::State(id) => world
                    .state(id)
                    .map_or(0.6, |state| crate::physics::ground_friction(&state.name)),
                Cell::Missing => 0.6,
            },
            |pos| match cell(world, origin, bounds, pos, air) {
                Cell::State(id) => world.state(id).map_or(0.0, |state| {
                    bounce_suppression
                        .as_ref()
                        .filter(|tag| !tag.contains(&state.name))
                        .map_or(0.0, |_| crate::items::block_bounce_restitution(&state.name))
                }),
                Cell::Missing => 0.0,
            },
            |pos| match cell(world, origin, bounds, pos, air) {
                Cell::State(id) => world.state(id).is_some_and(|state| {
                    state.name.ends_with("_wall") || state.name.ends_with("_fence_gate")
                }),
                Cell::Missing => false,
            },
            |item_bounds| {
                if fluid_view.chunks_loaded(item_bounds) {
                    fluid::contacts(
                        item_bounds,
                        item_bounds.min.y,
                        &|pos| fluid_view.at(pos),
                        &|pos, direction| fluid_view.sturdy_face(pos, direction),
                    )
                } else {
                    Default::default()
                }
            },
            self.fast_lava,
            |bounds, actual, requested| {
                fluid::swept_effects(bounds, actual, requested, &|pos| fluid_view.at(pos))
            },
        );
        update_recipe_unlocks(&mut self.inventory, &before_pickup, self.recipes.as_deref());
        self.tick_experience_orbs(self.death.is_none());
        if self.death.is_none() {
            self.food_use
                .tick(&mut self.inventory, &mut self.vitals, self.game_mode);
        }
        self.selected = selected_inventory_block(&self.inventory);
        self.first_person.item_use = self.food_use.visual_state();
        self.first_person.tick(&self.inventory);
        if self.death.is_none() {
            let effect = self.vitals.tick_food(self.survival_rules);
            self.vitals.apply_food_effect(effect, self.game_mode);
            if self.vitals.health <= 0.0 {
                self.death = Some(DeathInfo {
                    cause: DeathCause::Starve,
                    tick: self.tick.saturating_add(1),
                    score: self.score,
                    show_message: self.death_rules.show_death_messages,
                    inventory_unresolved: false,
                    respawn_pending: false,
                    can_respawn: false,
                });
            }
        }
        if self.death.is_some() {
            self.process_death();
            self.refresh_respawn_availability();
        }
        self.tick += 1;
        self.mark_advanced_saved_tick_chunks_dirty();
    }

    fn tick_experience_orbs(&mut self, alive: bool) {
        let world = &self.world;
        let shapes = &self.shapes;
        let origin = self.origin;
        let bounds = self.bounds;
        let air = self.states[0];
        let fluid_view = FluidView {
            world,
            shapes,
            fluids: &self.fluids,
            origin,
            bounds,
            air,
        };
        self.experience_orbs.tick_with_fluid(
            origin,
            self.motion.position,
            self.player.pose,
            self.game_mode,
            alive,
            &mut self.inventory,
            &mut self.player_random,
            &mut self.experience,
            &mut self.score,
            |pos| match cell(world, origin, bounds, pos, air) {
                Cell::Missing => unknown_barrier(),
                Cell::State(id) => shapes
                    .get(id.0 as usize)
                    .and_then(Option::as_deref)
                    .unwrap_or_else(|| unknown_barrier()),
            },
            |pos| match cell(world, origin, bounds, pos, air) {
                Cell::State(id) => world
                    .state(id)
                    .map_or(0.6, |state| crate::physics::ground_friction(&state.name)),
                Cell::Missing => 0.6,
            },
            |pos| match cell(world, origin, bounds, pos, air) {
                Cell::State(id) => world.state(id).is_some_and(|state| state.name.ends_with("_wall") || state.name.ends_with("_fence_gate")),
                Cell::Missing => false,
            },
            |orb_bounds, position| {
                if !fluid_view.chunks_loaded(orb_bounds) {
                    return (Default::default(), false);
                }
                let contacts = fluid::contacts(
                    orb_bounds,
                    position.y + f64::from(0.5_f32 * 0.85_f32),
                    &|pos| fluid_view.at(pos),
                    &|pos, direction| fluid_view.sturdy_face(pos, direction),
                );
                let lava = matches!(fluid_view.at(position.floor().as_ivec3()), fluid::Cell::Fluid(state) if state.kind == FluidKind::Lava);
                (contacts, lava)
            },
            self.fast_lava,
            |bounds, actual, requested| fluid::swept_effects(bounds, actual, requested, &|pos| fluid_view.at(pos)),
        );
    }

    fn process_death(&mut self) {
        if self.death_processed {
            return;
        }
        self.death_processed = true;
        self.experience_orbs.drop_player_experience(
            self.origin.as_dvec3() + self.motion.position,
            self.experience,
            self.death_rules.keep_inventory,
            self.game_mode,
        );
        self.respawn_death_safe = false;
        self.respawn_availability = None;
        self.world_respawn_needed = false;
        self.world_respawn_search = None;
        self.respawn_request_tick = None;
        self.respawn_target_ready = false;
        self.mining.release();
        self.food_use.cancel();
        if self.game_mode == GameMode::Spectator {
            return;
        }
        match Arc::make_mut(&mut self.inventory).take_death_drops(self.death_rules.keep_inventory) {
            Ok(drops) => {
                for stack in drops.inventory {
                    self.dropped_items.drop_death(
                        stack,
                        self.origin,
                        self.motion.position,
                        self.player,
                    );
                }
                for stack in drops.container_overflow {
                    self.dropped_items.drop_player(
                        stack,
                        self.origin,
                        self.motion.position,
                        self.player,
                        crate::items::PlayerDropOrigin::ContainerReturn,
                    );
                }
                self.selected = selected_inventory_block(&self.inventory);
                self.armor.invalidate();
                if let Err(error) = self.armor.refresh(&self.inventory) {
                    self.stream_errors
                        .push(format!("player armor equipment: {error:#}"));
                }
                self.respawn_death_safe = true;
            }
            Err(error) => {
                if let Some(death) = &mut self.death {
                    death.inventory_unresolved = true;
                }
                self.stream_errors
                    .push(format!("player death inventory preserved: {error:#}"));
            }
        }
    }
}

fn within_window(pos: ChunkPos, (center, radius): (ChunkPos, u32)) -> bool {
    let radius = i64::from(radius);
    (i64::from(pos.x) - i64::from(center.x)).abs() <= radius
        && (i64::from(pos.z) - i64::from(center.z)).abs() <= radius
}

fn validate_entity_chunk(pos: ChunkPos, data: &SavedEntityData) -> anyhow::Result<()> {
    if let SavedEntityData::Present(chunk) = data {
        anyhow::ensure!(
            chunk.position.x == pos.x && chunk.position.z == pos.z,
            "entity chunk position mismatch"
        );
    }
    Ok(())
}

impl GameSession for LoadedSession {
    fn submit_command(&mut self, command: PlayerCommand) {
        self.commands.push_back(command);
    }
    fn update(&mut self, delta: Duration) {
        self.accumulated = self.accumulated.saturating_add(delta);
        let mut processed = 0;
        while self.accumulated >= TICK && processed < MAX_TICKS_PER_UPDATE {
            self.accumulated -= TICK;
            self.advance_tick();
            processed += 1;
        }
        if processed > 0 {
            self.snapshot = Arc::new(WorldSnapshot {
                world: SessionWorld::Loaded(Arc::clone(&self.world)),
                player: self.player,
                camera: self.camera,
                game_mode: self.game_mode,
                vitals: self.vitals,
                experience: self.experience,
                death: self.death,
                armor: self.armor.value(),
                eyes_in_water: self.previous_eye_in_water,
                sprinting: self.sprint.active,
                selected_block: self.selected,
                inventory: Arc::clone(&self.inventory),
                first_person: self.first_person.clone(),
                breaking: self.mining.progress(),
                dropped_items: self.dropped_items.snapshot(),
                falling_blocks: self.falling_sand_snapshot(),
                experience_orbs: self.experience_orbs.snapshot(),
                system_messages: Arc::clone(&self.system_messages),
                tick: self.tick,
                origin: self.origin,
                world_revision: self.revision,
            });
        }
    }
    fn snapshot(&self) -> Arc<WorldSnapshot> {
        Arc::clone(&self.snapshot)
    }
    fn partial_tick(&self) -> f32 {
        (self.accumulated.as_secs_f32() / TICK.as_secs_f32()).clamp(0.0, 1.0)
    }
}

fn add(a: IVec3, b: IVec3) -> Option<IVec3> {
    Some(IVec3::new(
        a.x.checked_add(b.x)?,
        a.y.checked_add(b.y)?,
        a.z.checked_add(b.z)?,
    ))
}

fn subtract(a: IVec3, b: IVec3) -> Option<IVec3> {
    Some(IVec3::new(
        a.x.checked_sub(b.x)?,
        a.y.checked_sub(b.y)?,
        a.z.checked_sub(b.z)?,
    ))
}

#[derive(Clone, Copy)]
enum Cell {
    Missing,
    State(BlockStateId),
}

fn is_bed(name: &str) -> bool {
    name.strip_prefix("minecraft:")
        .is_some_and(|name| name.ends_with("_bed"))
}

struct LoadedDismountView<'a> {
    session: &'a LoadedSession,
    tags: &'a native_resources::respawn_tags::RespawnBlockTags,
    border: crate::respawn::RespawnBorder,
}

struct LoadedColumnSpawnView<'a> {
    session: &'a LoadedSession,
    ceiling_spawn_height: Option<i32>,
}

fn spawn_blocks_clear(session: &LoadedSession, bounds: BoundingBox) -> anyhow::Result<bool> {
    use anyhow::Context;
    const EPSILON: f64 = 1.0e-7;
    const LIQUID: [Aabb; 1] = [Aabb {
        min: glam::Vec3::ZERO,
        max: glam::Vec3::ONE,
    }];
    fn edge(value: f64, adjustment: i32) -> anyhow::Result<i32> {
        let floored = value.floor();
        anyhow::ensure!(
            floored >= f64::from(i32::MIN) && floored <= f64::from(i32::MAX),
            "spawn collision coordinate is out of range"
        );
        (floored as i32)
            .checked_add(adjustment)
            .context("spawn collision coordinate overflow")
    }
    let start = IVec3::new(
        edge(bounds.min.x - EPSILON, -1)?,
        edge(bounds.min.y - EPSILON, -1)?,
        edge(bounds.min.z - EPSILON, -1)?,
    );
    let end = IVec3::new(
        edge(bounds.max.x + EPSILON, 1)?,
        edge(bounds.max.y + EPSILON, 1)?,
        edge(bounds.max.z + EPSILON, 1)?,
    );
    let mut unavailable = None;
    // Java's Cursor3D skips corners and only tests large shapes on its outer
    // faces. Keep an unknown result until all known collisions are examined.
    for z in start.z..=end.z {
        for y in start.y..=end.y {
            for x in start.x..=end.x {
                let edge_count = u8::from(x == start.x || x == end.x)
                    + u8::from(y == start.y || y == end.y)
                    + u8::from(z == start.z || z == end.z);
                if edge_count == 3 {
                    continue;
                }
                let pos = IVec3::new(x, y, z);
                let query = (|| -> anyhow::Result<bool> {
                    let id = session.respawn_state_id(pos)?;
                    let state = session
                        .world
                        .state(id)
                        .context("unknown spawn collision state")?;
                    if edge_count == 2 && state.name != "minecraft:moving_piston" {
                        return Ok(false);
                    }
                    let normal = session
                        .shapes
                        .get(id.0 as usize)
                        .and_then(Option::as_deref)
                        .context("spawn collision shape is unavailable")?;
                    if edge_count == 1
                        && !normal.iter().any(|shape| {
                            shape.min.cmplt(glam::Vec3::ZERO).any()
                                || shape.max.cmpgt(glam::Vec3::ONE).any()
                        })
                    {
                        return Ok(false);
                    }
                    session
                        .heightmap_flags
                        .get(id.0 as usize)
                        .copied()
                        .flatten()
                        .context("spawn block state behavior is unavailable")?;
                    let shapes =
                        if matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
                            &LIQUID[..]
                        } else {
                            normal
                        };
                    let full = shapes.len() == 1
                        && shapes[0].min == glam::Vec3::ZERO
                        && shapes[0].max == glam::Vec3::ONE;
                    let tolerance = if full { 0.0 } else { EPSILON };
                    let base = pos.as_dvec3();
                    Ok(shapes.iter().any(|shape| {
                        let min = base + shape.min.as_dvec3();
                        let max = base + shape.max.as_dvec3();
                        bounds.max.cmpgt(min + DVec3::splat(tolerance)).all()
                            && bounds.min.cmplt(max - DVec3::splat(tolerance)).all()
                    }))
                })();
                match query {
                    Ok(true) => return Ok(false),
                    Ok(false) => {}
                    Err(error) => {
                        if unavailable.is_none() {
                            unavailable = Some(error);
                        }
                    }
                }
            }
        }
    }
    if let Some(error) = unavailable {
        return Err(error);
    }
    Ok(true)
}

impl crate::world_spawn::SharedSpawnWorld for LoadedColumnSpawnView<'_> {
    fn min_y(&self) -> i32 {
        self.session.bounds.min_y
    }
    fn max_y(&self) -> i32 {
        self.session.bounds.max_y - 1
    }
    fn ceiling_spawn_height(&self) -> Option<i32> {
        self.ceiling_spawn_height
    }
    fn heights(
        &mut self,
        x: i32,
        z: i32,
    ) -> anyhow::Result<crate::world_spawn::SpawnColumnHeights> {
        self.session.spawn_column_heights(x, z)
    }
    fn has_fluid(&mut self, pos: IVec3) -> anyhow::Result<bool> {
        use anyhow::Context;
        let id = self.session.respawn_state_id(pos)?;
        Ok(self
            .session
            .heightmap_flags
            .get(id.0 as usize)
            .copied()
            .flatten()
            .context("spawn block fluid state is unavailable")?
            .has_fluid)
    }
    fn full_top_face(&mut self, pos: IVec3) -> anyhow::Result<bool> {
        use anyhow::Context;
        let id = self.session.respawn_state_id(pos)?;
        let shapes = self
            .session
            .shapes
            .get(id.0 as usize)
            .and_then(Option::as_deref)
            .context("spawn block collision shape is unavailable")?;
        if shapes.is_empty() {
            return Ok(false);
        }
        if shapes.len() == 1
            && shapes[0].min == glam::Vec3::ZERO
            && shapes[0].max == glam::Vec3::ONE
        {
            return Ok(true);
        }
        Ok(covers_full_face(shapes, IVec3::Y))
    }
    fn clear_player(&mut self, pos: IVec3) -> anyhow::Result<bool> {
        self.session.world_spawn_clear(pos)
    }
}

struct LoadedSavedSpawnView<'a, F> {
    dismount: LoadedDismountView<'a>,
    environment: native_resources::respawn_environment::RespawnEnvironment,
    dark_outside: bool,
    possible: &'a mut F,
}

impl<F> crate::respawn::SavedSpawnWorld for LoadedSavedSpawnView<'_, F>
where
    F: FnMut(&BlockState) -> anyhow::Result<bool>,
{
    type Error = anyhow::Error;

    fn spawn_block(&mut self, pos: IVec3) -> anyhow::Result<crate::respawn::SavedSpawnBlock> {
        use crate::respawn::SavedSpawnBlock;
        let state = self.dismount.state_at(pos)?;
        if is_bed(&state.name) {
            Ok(SavedSpawnBlock::Bed {
                straw: state.name == "minecraft:straw_bed",
            })
        } else if state.name == "minecraft:respawn_anchor" {
            let charges = state
                .properties
                .get("charges")
                .and_then(|value| value.parse::<u8>().ok())
                .filter(|value| *value <= 4)
                .ok_or_else(|| anyhow::anyhow!("respawn anchor charge is missing or invalid"))?;
            Ok(SavedSpawnBlock::Anchor { charges })
        } else {
            Ok(SavedSpawnBlock::Other)
        }
    }

    fn bed_can_set_spawn(&mut self, _pos: IVec3, straw: bool) -> anyhow::Result<bool> {
        let rule = if straw {
            self.environment.straw_bed
        } else {
            self.environment.bed
        };
        Ok(rule.can_set_spawn.test(self.dark_outside))
    }

    fn anchor_works(&mut self, _pos: IVec3) -> anyhow::Result<bool> {
        Ok(self.environment.anchor_works)
    }

    fn bed_stand_up(&mut self, pos: IVec3, yaw: f32) -> anyhow::Result<Option<DVec3>> {
        self.dismount.session.bed_stand_up_position(
            pos,
            yaw,
            self.dismount.tags,
            self.dismount.border,
        )
    }

    fn anchor_stand_up(&mut self, pos: IVec3) -> anyhow::Result<Option<DVec3>> {
        self.dismount.session.anchor_stand_up_position(
            pos,
            self.dismount.tags,
            self.dismount.border,
        )
    }

    fn possible_to_respawn_in(&mut self, pos: IVec3) -> anyhow::Result<bool> {
        let state = self.dismount.state_at(pos)?;
        (self.possible)(state)
    }
}

impl<'a> LoadedDismountView<'a> {
    fn new(
        session: &'a LoadedSession,
        tags: &'a native_resources::respawn_tags::RespawnBlockTags,
        border: crate::respawn::RespawnBorder,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(border.is_valid(), "world border is invalid");
        Ok(Self {
            session,
            tags,
            border,
        })
    }

    fn state_id(&self, pos: IVec3) -> anyhow::Result<BlockStateId> {
        let id = self.session.respawn_state_id(pos)?;
        let state = self
            .session
            .world
            .state(id)
            .ok_or_else(|| anyhow::anyhow!("unknown respawn block state"))?;
        anyhow::ensure!(
            self.tags.contains_block(&state.name),
            "block {} is absent from the respawn registry",
            state.name
        );
        Ok(id)
    }

    fn state_at(&self, pos: IVec3) -> anyhow::Result<&BlockState> {
        let id = self.state_id(pos)?;
        let state = self
            .session
            .world
            .state(id)
            .ok_or_else(|| anyhow::anyhow!("unknown respawn block state"))?;
        Ok(state)
    }

    fn shape_at(&self, pos: IVec3) -> anyhow::Result<&[Aabb]> {
        let id = self.state_id(pos)?;
        self.session
            .shapes
            .get(id.0 as usize)
            .and_then(Option::as_deref)
            .ok_or_else(|| anyhow::anyhow!("unsupported respawn collision shape at {pos:?}"))
    }

    fn facts_at(
        &self,
        pos: IVec3,
    ) -> anyhow::Result<native_resources::respawn_tags::RespawnBlockFacts> {
        let state = self.state_at(pos)?;
        self.tags.for_block(&state.name, &state.properties)
    }
}

impl crate::respawn::DismountWorld for LoadedDismountView<'_> {
    type Error = anyhow::Error;

    fn is_dangerous(&mut self, pos: IVec3) -> anyhow::Result<bool> {
        Ok(self.facts_at(pos)?.dangerous)
    }

    fn non_climbable_top(&mut self, pos: IVec3) -> anyhow::Result<Option<f64>> {
        let state = self.state_at(pos)?;
        if self.facts_at(pos)?.climbable {
            return Ok(None);
        }
        if state.name.ends_with("_trapdoor") {
            match state.properties.get("open").map(String::as_str) {
                Some("true") => return Ok(None),
                Some("false") => {}
                _ => anyhow::bail!("trapdoor open state is missing or invalid"),
            }
        }
        Ok(self
            .shape_at(pos)?
            .iter()
            .map(|shape| f64::from(shape.max.y))
            .reduce(f64::max))
    }

    fn has_block_collision(&mut self, bounds: BoundingBox) -> anyhow::Result<bool> {
        const EPSILON: f64 = 1.0e-7;
        let start = (bounds.min - DVec3::splat(EPSILON)).floor().as_ivec3();
        let end = (bounds.max + DVec3::splat(EPSILON)).floor().as_ivec3();
        // Adjacent blocks may have collision shapes extending past their cell.
        for x in start.x.saturating_sub(1)..=end.x.saturating_add(1) {
            for y in start.y.saturating_sub(1)..=end.y.saturating_add(1) {
                for z in start.z.saturating_sub(1)..=end.z.saturating_add(1) {
                    let pos = IVec3::new(x, y, z);
                    let shapes = self.shape_at(pos)?;
                    let full = shapes.len() == 1
                        && shapes[0].min == glam::Vec3::ZERO
                        && shapes[0].max == glam::Vec3::ONE;
                    let tolerance = if full { 0.0 } else { EPSILON };
                    let base = pos.as_dvec3();
                    for shape in shapes {
                        let min = base + shape.min.as_dvec3();
                        let max = base + shape.max.as_dvec3();
                        if bounds.max.cmpgt(min + DVec3::splat(tolerance)).all()
                            && bounds.min.cmplt(max - DVec3::splat(tolerance)).all()
                        {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    }

    fn invalid_spawn_inside(&mut self, pos: IVec3) -> anyhow::Result<bool> {
        Ok(self.facts_at(pos)?.invalid_inside)
    }

    fn within_world_border(&mut self, bounds: BoundingBox) -> anyhow::Result<bool> {
        Ok(self.border.contains(bounds))
    }
}

/// Only classify solidness for blocks whose ordinary behavior is known.
fn known_soil_cover(name: &str) -> Option<bool> {
    if is_vanilla_fence_gate(name) {
        Some(true)
    } else if matches!(
        name,
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:water"
            | "minecraft:lava"
    ) {
        Some(false)
    } else {
        Block::from_item_id(name).map(Block::is_solid)
    }
}

fn is_vanilla_fence_gate(name: &str) -> bool {
    let Some(family) = name
        .strip_prefix("minecraft:")
        .and_then(|id| id.strip_suffix("_fence_gate"))
    else {
        return false;
    };
    native_core::wood::Planks::ALL.into_iter().any(|wood| {
        wood.id()
            .strip_prefix("minecraft:")
            .and_then(|id| id.strip_suffix("_planks"))
            == Some(family)
    })
}

fn stone_button_state(face: IVec3, yaw: f32) -> Option<BlockState> {
    if !yaw.is_finite() {
        return None;
    }
    let java_yaw = f64::from(yaw.to_degrees() - 180.0);
    let horizontal =
        ["south", "west", "north", "east"][(((java_yaw / 90.0 + 0.5).floor() as i32) & 3) as usize];
    let (attachment, facing) = match face {
        IVec3::Y => ("floor", horizontal),
        IVec3::NEG_Y => ("ceiling", horizontal),
        IVec3::X => ("wall", "east"),
        IVec3::NEG_X => ("wall", "west"),
        IVec3::Z => ("wall", "south"),
        IVec3::NEG_Z => ("wall", "north"),
        _ => return None,
    };
    Some(state(
        "minecraft:stone_button",
        &[
            ("face", attachment),
            ("facing", facing),
            ("powered", "false"),
        ],
    ))
}

/// These default block behaviors were checked with a stone-button placement
/// context. Other replaceable states can have item-specific rules.
fn stone_button_can_replace(block: &BlockState) -> Option<bool> {
    let id = block.name.strip_prefix("minecraft:")?;
    if matches!(
        id,
        "air"
            | "cave_air"
            | "void_air"
            | "water"
            | "lava"
            | "fire"
            | "soul_fire"
            | "short_grass"
            | "fern"
            | "dead_bush"
    ) {
        return Some(true);
    }
    if id.ends_with("_slab") || id.ends_with("_stairs") || id.ends_with("_button") {
        return Some(false);
    }
    if button_support_face(block, IVec3::Y) == Some(true) {
        return Some(false);
    }
    None
}

/// Java's Direction.orderedByNearest with the clicked face's opposite moved
/// to the front by BlockPlaceContext when placing in the adjacent cell.
fn button_placement_directions(
    yaw: f32,
    pitch: f32,
    face: IVec3,
    replace_clicked: bool,
) -> Option<[IVec3; 6]> {
    if !yaw.is_finite()
        || !pitch.is_finite()
        || !matches!(
            face,
            IVec3::X | IVec3::NEG_X | IVec3::Y | IVec3::NEG_Y | IVec3::Z | IVec3::NEG_Z
        )
    {
        return None;
    }
    let pitch_rad = -pitch.to_degrees() * 0.017453292_f32;
    let yaw_rad = -(yaw.to_degrees() - 180.0) * 0.017453292_f32;
    let sin_pitch = crate::physics::java_sin(f64::from(pitch_rad), false);
    let cos_pitch = crate::physics::java_sin(f64::from(pitch_rad), true);
    let sin_yaw = crate::physics::java_sin(f64::from(yaw_rad), false);
    let cos_yaw = crate::physics::java_sin(f64::from(yaw_rad), true);
    let x = if sin_yaw > 0.0 {
        IVec3::X
    } else {
        IVec3::NEG_X
    };
    let y = if sin_pitch < 0.0 {
        IVec3::Y
    } else {
        IVec3::NEG_Y
    };
    let z = if cos_yaw > 0.0 {
        IVec3::Z
    } else {
        IVec3::NEG_Z
    };
    let ax = sin_yaw.abs();
    let ay = sin_pitch.abs();
    let az = cos_yaw.abs();
    let (a, b, c) = if ax > az {
        if ay > ax * cos_pitch {
            (y, x, z)
        } else if az * cos_pitch > ay {
            (x, z, y)
        } else {
            (x, y, z)
        }
    } else if ay > az * cos_pitch {
        (y, z, x)
    } else if ax * cos_pitch > ay {
        (z, x, y)
    } else {
        (z, y, x)
    };
    let mut directions = [a, b, c, -c, -b, -a];
    if !replace_clicked {
        let index = directions
            .iter()
            .position(|direction| *direction == -face)?;
        directions[..=index].rotate_right(1);
    }
    Some(directions)
}

fn stone_button_support_direction(button: &BlockState) -> Option<IVec3> {
    if button.name != "minecraft:stone_button" || block_outline(button).is_none() {
        return None;
    }
    match button.properties.get("face")?.as_str() {
        "floor" => Some(IVec3::NEG_Y),
        "ceiling" => Some(IVec3::Y),
        "wall" => match button.properties.get("facing")?.as_str() {
            "north" => Some(IVec3::Z),
            "south" => Some(IVec3::NEG_Z),
            "west" => Some(IVec3::X),
            "east" => Some(IVec3::NEG_X),
            _ => None,
        },
        _ => None,
    }
}

fn cell(
    world: &World,
    origin: IVec3,
    bounds: BuildBounds,
    relative: IVec3,
    air: BlockStateId,
) -> Cell {
    let Some(pos) = add(origin, relative) else {
        return Cell::Missing;
    };
    let Some(chunk) = world.chunks.get(&ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    }) else {
        return Cell::Missing;
    };
    if chunk.status.as_deref() != Some("minecraft:full") {
        return Cell::Missing;
    }
    if pos.y < bounds.min_y || pos.y >= bounds.max_y {
        return Cell::State(air);
    }
    // Full chunks may omit all-air sections or retain sections with light alone.
    Cell::State(world.block_id(pos.x, pos.y, pos.z).unwrap_or(air))
}

/// The fluid algorithms need block tags and support faces. Until the complete
/// registry is available, collision shapes supply a conservative approximation.
struct FluidView<'a> {
    world: &'a World,
    shapes: &'a [Option<Vec<Aabb>>],
    fluids: &'a [Option<FluidState>],
    origin: IVec3,
    bounds: BuildBounds,
    air: BlockStateId,
}

impl FluidView<'_> {
    fn at(&self, pos: IVec3) -> fluid::Cell {
        match cell(self.world, self.origin, self.bounds, pos, self.air) {
            Cell::Missing => fluid::Cell::Missing,
            Cell::State(id) => {
                if let Some(state) = self.fluids.get(id.0 as usize).copied().flatten() {
                    return fluid::Cell::Fluid(state);
                }
                let sign = self
                    .world
                    .state(id)
                    .is_some_and(|state| state.name.ends_with("_sign"));
                let blocks_flow = self
                    .shapes
                    .get(id.0 as usize)
                    .and_then(Option::as_deref)
                    .is_none_or(|shape| !shape.is_empty());
                if sign || blocks_flow {
                    fluid::Cell::Solid
                } else {
                    fluid::Cell::Empty
                }
            }
        }
    }

    fn sturdy_face(&self, pos: IVec3, direction: IVec3) -> bool {
        match cell(self.world, self.origin, self.bounds, pos, self.air) {
            Cell::Missing => true,
            Cell::State(id) => {
                if direction.y == 0
                    && self.world.state(id).is_some_and(|state| {
                        matches!(
                            state.name.as_str(),
                            "minecraft:ice" | "minecraft:frosted_ice"
                        )
                    })
                {
                    return false;
                }
                let shape = self
                    .shapes
                    .get(id.0 as usize)
                    .and_then(Option::as_deref)
                    .unwrap_or_else(|| unknown_barrier());
                covers_full_face(shape, direction)
            }
        }
    }

    /// Vanilla samples only when all chunks touching the interaction box and
    /// its horizontal one-block border are loaded.
    fn chunks_loaded(&self, bounds: BoundingBox) -> bool {
        let min = bounds.min + DVec3::splat(0.001);
        let max = bounds.max - DVec3::splat(0.001);
        let Some(min_x) = i64::from(self.origin.x)
            .checked_add(min.x.floor() as i64)
            .and_then(|x| x.checked_sub(1))
        else {
            return false;
        };
        let Some(min_z) = i64::from(self.origin.z)
            .checked_add(min.z.floor() as i64)
            .and_then(|z| z.checked_sub(1))
        else {
            return false;
        };
        let Some(max_x) = i64::from(self.origin.x).checked_add(max.x.ceil() as i64) else {
            return false;
        };
        let Some(max_z) = i64::from(self.origin.z).checked_add(max.z.ceil() as i64) else {
            return false;
        };
        for x in min_x.div_euclid(16)..=max_x.div_euclid(16) {
            for z in min_z.div_euclid(16)..=max_z.div_euclid(16) {
                let (Ok(x), Ok(z)) = (i32::try_from(x), i32::try_from(z)) else {
                    return false;
                };
                if !self
                    .world
                    .chunks
                    .get(&ChunkPos { x, z })
                    .is_some_and(|chunk| chunk.status.as_deref() == Some("minecraft:full"))
                {
                    return false;
                }
            }
        }
        true
    }

    fn contains_liquid(&self, bounds: BoundingBox) -> bool {
        let start = bounds.min.floor().as_ivec3();
        let end = bounds.max.floor().as_ivec3();
        for x in start.x..=end.x {
            for y in start.y..=end.y {
                for z in start.z..=end.z {
                    if matches!(
                        self.at(IVec3::new(x, y, z)),
                        fluid::Cell::Fluid(_) | fluid::Cell::Missing
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn move_player(
    world: &World,
    shapes: &[Option<Vec<Aabb>>],
    origin: IVec3,
    bounds: BuildBounds,
    air: BlockStateId,
    position: DVec3,
    movement: DVec3,
    on_ground: bool,
) -> DVec3 {
    crate::movement::resolve_player(position, movement, on_ground, PLAYER_HEIGHT, |cell_pos| {
        match cell(world, origin, bounds, cell_pos, air) {
            Cell::Missing => unknown_barrier(),
            Cell::State(id) => shapes
                .get(id.0 as usize)
                .and_then(Option::as_deref)
                .unwrap_or_else(|| unknown_barrier()),
        }
    })
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn move_axis(
    world: &World,
    shapes: &[Option<Vec<Aabb>>],
    origin: IVec3,
    bounds: BuildBounds,
    air: BlockStateId,
    position: &mut Vec3,
    axis: usize,
    amount: f32,
) -> bool {
    let mut requested = Vec3::ZERO;
    requested[axis] = amount;
    let actual = move_player(
        world,
        shapes,
        origin,
        bounds,
        air,
        position.as_dvec3(),
        requested.as_dvec3(),
        false,
    );
    *position = (position.as_dvec3() + actual).as_vec3();
    actual[axis] != f64::from(amount)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PLAYER_HALF_WIDTH;
    use native_world::loaded::{Chunk, LIGHT_BYTES};
    use std::collections::BTreeMap;

    fn fixture() -> (World, IVec3, PlayerState) {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("minecraft:air", &[]));
        let stone = world.intern_state(state("minecraft:stone", &[]));
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        for x in 0..16 {
            for z in 0..16 {
                blocks[x + z * 16] = stone;
            }
        }
        blocks[8 + 6 * 16 + 2 * 256] = stone;
        let pos = ChunkPos {
            x: 1_000_000,
            z: -1_000_000,
        };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:full".into()),
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
        let origin = IVec3::new(pos.x * 16, 0, pos.z * 16);
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: PlayerPose::Standing,
        };
        (world, origin, player)
    }

    fn session() -> LoadedSession {
        let (world, origin, player) = fixture();
        let mut session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session.set_falling_fire_tag(Arc::new(BTreeSet::new()));
        session.set_bounce_suppression_tag(Arc::new(BTreeSet::new()));
        session.set_farmland_maintainers(Arc::new(BTreeSet::new()));
        session
    }

    #[test]
    fn player_friction_tracks_support_at_edges_and_across_rebase() {
        let mut session = session();
        let ice = session.intern_block(state("minecraft:ice", &[]));
        assert!(session.set(IVec3::new(4, 0, 4), ice));
        assert!(session.set(IVec3::new(5, 0, 4), session.states[0]));
        session.motion.position = DVec3::new(5.05, 1.0, 4.5);
        session.motion.velocity = DVec3::ZERO;
        session.motion.on_ground = false;
        session.player.position = session.motion.position.as_vec3();
        session.player.on_ground = false;
        session.update(TICK * 2);
        assert!(session.motion.on_ground);
        assert_eq!(
            session.ground_support,
            Some(session.origin + IVec3::new(4, 0, 4))
        );
        assert_eq!(session.friction_below(), 0.98);
        session.motion.velocity.z = 0.1;
        session.update(TICK);
        let expected = 0.1 * f64::from(0.98_f32 * 0.91_f32);
        assert!((session.motion.velocity.z - expected).abs() < 1.0e-10);

        let slime = session.intern_block(state("minecraft:slime_block", &[]));
        assert!(session.set(IVec3::new(4, 0, 4), slime));
        assert_eq!(session.friction_below(), 0.8);
        assert!(session.set(IVec3::new(4, 0, 4), ice));

        let support = session.ground_support;
        session.origin.x += 16;
        session.motion.position.x -= 16.0;
        assert_eq!(session.ground_support, support);
        assert_eq!(session.friction_below(), 0.98);
    }

    #[test]
    fn player_bounces_on_slime_unless_sneaking_or_suppressed() {
        for (name, sneaking, suppressed, bounces, start_y) in [
            ("minecraft:slime_block", false, false, true, 2.2),
            ("minecraft:slime_block", true, false, false, 2.2),
            ("minecraft:slime_block", false, true, false, 2.2),
            ("minecraft:stone", false, false, false, 2.2),
            ("minecraft:red_bed", false, false, true, 2.0),
        ] {
            let mut session = session();
            let block = session.intern_block(if name == "minecraft:red_bed" {
                state(
                    name,
                    &[("facing", "north"), ("part", "head"), ("occupied", "false")],
                )
            } else {
                state(name, &[])
            });
            assert!(session.set(IVec3::new(4, 1, 4), block));
            if suppressed {
                session.set_bounce_suppression_tag(Arc::new(BTreeSet::from([name.into()])));
            }
            session.motion.position = DVec3::new(4.5, start_y, 4.5);
            session.motion.velocity = DVec3::new(0.0, -0.5, 0.0);
            session.motion.on_ground = false;
            session.player.position = session.motion.position.as_vec3();
            session.player.on_ground = false;
            session.sneak = sneaking;
            session.update(TICK);
            assert!(session.motion.on_ground, "{name}");
            assert_eq!(
                session.motion.velocity.y > 0.0,
                bounces,
                "{name} sneaking={sneaking} suppressed={suppressed}"
            );
            let landed = session.motion.position.y;
            session.update(TICK);
            assert_eq!(session.motion.position.y > landed + 1e-6, bounces, "{name}");
        }
    }

    #[test]
    fn player_bounciness_tracks_saved_and_equipped_modifiers() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "attributes".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:bounciness".into())),
                        ("base".into(), Tag::Double(0.1)),
                    ])],
                },
            )]),
        };
        let mut session = session();
        session
            .set_saved_bounciness(
                crate::player_data::read_saved_bounciness_attribute(&document).unwrap(),
            )
            .unwrap();
        let component = Tag::Compound(vec![(
            "minecraft:attribute_modifiers".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(vec![
                    ("type".into(), Tag::String("minecraft:bounciness".into())),
                    ("id".into(), Tag::String("example:bounce".into())),
                    ("operation".into(), Tag::String("add_value".into())),
                    ("amount".into(), Tag::Double(0.4)),
                    ("slot".into(), Tag::String("head".into())),
                ])],
            },
        )]);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            crate::inventory::PlayerSlot::Armor(0),
            ItemStack::unresolved("minecraft:iron_helmet".into(), 1, component),
        );
        session.set_player_inventory(inventory);
        session.refresh_bounciness();
        assert_eq!(session.bounciness, Some(0.5));
        let mut replacement = PlayerInventory::default();
        replacement.set_slot(
            crate::inventory::PlayerSlot::Armor(0),
            ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])),
        );
        assert_eq!(replacement.revision(), session.inventory.revision());
        session.set_player_inventory(replacement);
        session.refresh_bounciness();
        assert_eq!(session.bounciness, Some(0.1));
        Arc::make_mut(&mut session.inventory)
            .set_slot(crate::inventory::PlayerSlot::Armor(0), None);
        session.refresh_bounciness();
        assert_eq!(session.bounciness, Some(0.1));
    }

    #[test]
    fn jump_strength_tracks_saved_and_equipped_modifiers() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "attributes".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:jump_strength".into())),
                        ("base".into(), Tag::Double(0.3)),
                    ])],
                },
            )]),
        };
        let mut session = session();
        session
            .set_saved_jump_strength(
                crate::player_data::read_saved_jump_strength_attribute(&document).unwrap(),
            )
            .unwrap();
        let component = Tag::Compound(vec![(
            "minecraft:attribute_modifiers".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(vec![
                    ("type".into(), Tag::String("minecraft:jump_strength".into())),
                    ("id".into(), Tag::String("example:jump".into())),
                    ("operation".into(), Tag::String("add_value".into())),
                    ("amount".into(), Tag::Double(0.4)),
                    ("slot".into(), Tag::String("head".into())),
                ])],
            },
        )]);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            crate::inventory::PlayerSlot::Armor(0),
            ItemStack::unresolved("minecraft:leather_helmet".into(), 1, component),
        );
        session.set_player_inventory(inventory);
        session.refresh_jump_strength();
        assert!((session.jump_strength.unwrap() - 0.7).abs() < 1e-12);
        Arc::make_mut(&mut session.inventory)
            .set_slot(crate::inventory::PlayerSlot::Armor(0), None);
        session.refresh_jump_strength();
        assert_eq!(session.jump_strength, Some(0.3));
    }

    #[test]
    fn movement_speed_tracks_saved_effect_modifiers_and_their_transitions() {
        use native_resources::{
            enchantments::{AttributeModifier, AttributeOperation},
            packs::ResourceId,
        };
        let mut session = session();
        let effects = crate::survival::FallEffects {
            speed: Some(crate::survival::EffectDuration {
                amplifier: 1,
                duration: 1,
                hidden: Some(Box::new(crate::survival::EffectDuration {
                    amplifier: 0,
                    duration: 4,
                    hidden: None,
                })),
            }),
            slowness: Some(crate::survival::EffectDuration {
                amplifier: 0,
                duration: 2,
                hidden: None,
            }),
            ..Default::default()
        };
        session.fall.configure(
            crate::player_data::SavedAttribute::with_base(3.0),
            crate::player_data::SavedAttribute::with_base(1.0),
            effects,
            Arc::new(OxygenEnchantments::default()),
        );
        let mut saved = crate::player_data::SavedAttribute::with_base(f64::from(0.1_f32));
        for (id, amount) in [
            ("minecraft:effect.speed", f64::from(0.2_f32) * 2.0),
            ("minecraft:effect.slowness", f64::from(-0.15_f32)),
        ] {
            saved.apply(AttributeModifier {
                id: ResourceId::parse(id).unwrap(),
                amount,
                operation: AttributeOperation::AddMultipliedTotal,
            });
        }
        session.set_saved_movement_speed(saved).unwrap();
        session.update(TICK);
        let promoted =
            (f64::from(0.1_f32) * (1.0 + f64::from(-0.15_f32)) * (1.0 + f64::from(0.2_f32))) as f32;
        assert!((session.movement_speed(false) - promoted).abs() < 1e-7);
        session.update(TICK);
        assert!((session.movement_speed(false) - 0.12).abs() < 1e-7);
        session.update(TICK);
        session.update(TICK);
        assert_eq!(session.movement_speed(false), 0.1_f32);

        // Java loads active effects and persistent attributes separately: it
        // does not invent a missing effect modifier until a transition occurs.
        let mut absent = self::session();
        absent.fall.configure(
            crate::player_data::SavedAttribute::with_base(3.0),
            crate::player_data::SavedAttribute::with_base(1.0),
            crate::survival::FallEffects {
                speed: Some(crate::survival::EffectDuration {
                    amplifier: 3,
                    duration: 10,
                    hidden: None,
                }),
                ..Default::default()
            },
            Arc::new(OxygenEnchantments::default()),
        );
        absent.refresh_movement_speed();
        assert_eq!(absent.movement_speed(false), 0.1_f32);
    }

    #[test]
    fn player_draft_updates_promoted_and_expired_speed_modifiers() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let modifier = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:effect.speed".into())),
            ("amount".into(), Tag::Double(f64::from(0.2_f32) * 2.0)),
            (
                "operation".into(),
                Tag::String("add_multiplied_total".into()),
            ),
        ]);
        let unrelated = Tag::Compound(vec![
            ("id".into(), Tag::String("example:keep".into())),
            ("amount".into(), Tag::Double(0.25)),
            ("operation".into(), Tag::String("add_value".into())),
            ("extra".into(), Tag::Int(37)),
        ]);
        let original = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "active_effects".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:speed".into())),
                            ("duration".into(), Tag::Int(1)),
                            ("amplifier".into(), Tag::Byte(1)),
                            (
                                "hidden_effect".into(),
                                Tag::Compound(vec![("duration".into(), Tag::Int(4))]),
                            ),
                        ])],
                    },
                ),
                (
                    "attributes".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:movement_speed".into())),
                            ("base".into(), Tag::Double(f64::from(0.1_f32))),
                            ("custom".into(), Tag::Int(12)),
                            (
                                "modifiers".into(),
                                Tag::List {
                                    element_type: TagType::Compound,
                                    values: vec![modifier, unrelated.clone()],
                                },
                            ),
                        ])],
                    },
                ),
            ]),
        };
        let before = native_persistence::nbt::write(&original).unwrap();
        let mut session = session();
        session.fall.configure(
            crate::player_data::SavedAttribute::with_base(3.0),
            crate::player_data::SavedAttribute::with_base(1.0),
            crate::player_data::read_saved_fall_effects(&original).unwrap(),
            Arc::new(OxygenEnchantments::default()),
        );
        session
            .set_saved_movement_speed(
                crate::player_data::read_saved_movement_speed_attribute(&original).unwrap(),
            )
            .unwrap();
        session.update(TICK);
        let draft = session.draft_player_state(&original).unwrap();
        let Tag::List { values, .. } = draft.document().value.get("attributes").unwrap() else {
            panic!()
        };
        assert_eq!(values[0].get("custom"), Some(&Tag::Int(12)));
        let Tag::List {
            values: modifiers, ..
        } = values[0].get("modifiers").unwrap()
        else {
            panic!()
        };
        assert!(modifiers.contains(&unrelated));
        assert!(
            modifiers
                .iter()
                .any(|m| m.get("amount") == Some(&Tag::Double(f64::from(0.2_f32))))
        );
        assert_eq!(native_persistence::nbt::write(&original).unwrap(), before);
        for _ in 0..3 {
            session.update(TICK);
        }
        let expired = session.draft_player_state(&original).unwrap();
        let Tag::List { values, .. } = expired.document().value.get("attributes").unwrap() else {
            panic!()
        };
        let Tag::List {
            values: modifiers, ..
        } = values[0].get("modifiers").unwrap()
        else {
            panic!()
        };
        assert_eq!(modifiers, &[unrelated]);

        let mut changed = original.clone();
        if let Tag::Compound(fields) = &mut changed.value {
            if let Tag::List { values, .. } = &mut fields
                .iter_mut()
                .find(|(n, _)| n.matches("attributes"))
                .unwrap()
                .1
            {
                if let Tag::Compound(entries) = &mut values[0] {
                    if let Tag::List { values, .. } = &mut entries
                        .iter_mut()
                        .find(|(n, _)| n.matches("modifiers"))
                        .unwrap()
                        .1
                    {
                        if let Tag::Compound(fields) = &mut values[0] {
                            fields.push(("unknown".into(), Tag::Int(1)));
                        }
                    }
                }
            }
        }
        assert!(session.draft_player_state(&changed).is_err());
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn movement_effect_drafts_match_vanilla_attribute_transitions() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        use native_resources::packs::ResourceId;
        use std::{path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(|path| PathBuf::from(path).join("bin"))
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin"));
        let temp = tempfile::tempdir().unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(game.join("versions/26.3/26.3.json")).unwrap())
                .unwrap();
        let mut paths = vec![
            game.join("versions/26.3/26.3.jar"),
            temp.path().to_path_buf(),
        ];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let effect = |id: &str, duration, amplifier, hidden| {
            let mut fields = vec![
                ("id".into(), Tag::String(id.into())),
                ("duration".into(), Tag::Int(duration)),
                ("amplifier".into(), Tag::Byte(amplifier)),
            ];
            if let Some(duration) = hidden {
                fields.push((
                    "hidden_effect".into(),
                    Tag::Compound(vec![("duration".into(), Tag::Int(duration))]),
                ));
            }
            Tag::Compound(fields)
        };
        let modifier = |id: &str, amount| {
            Tag::Compound(vec![
                ("id".into(), Tag::String(id.into())),
                ("amount".into(), Tag::Double(amount)),
                (
                    "operation".into(),
                    Tag::String("add_multiplied_total".into()),
                ),
            ])
        };
        let scenarios = vec![
            (
                vec![effect("minecraft:speed", 1, 1, Some(4))],
                Some(vec![modifier(
                    "minecraft:effect.speed",
                    f64::from(0.2_f32) * 2.0,
                )]),
            ),
            (vec![effect("minecraft:speed", 1, 1, Some(4))], None),
            (
                vec![effect("minecraft:slowness", 2, 0, None)],
                Some(vec![modifier(
                    "minecraft:effect.slowness",
                    f64::from(-0.15_f32),
                )]),
            ),
            (
                vec![
                    effect("minecraft:speed", -1, 1, None),
                    effect("minecraft:slowness", 3, 0, None),
                ],
                Some(vec![
                    modifier("minecraft:effect.speed", f64::from(0.2_f32) * 2.0),
                    modifier("minecraft:effect.slowness", f64::from(-0.15_f32)),
                ]),
            ),
            (vec![effect("minecraft:speed", 0, 0, None)], None),
        ];
        let mut documents = Vec::new();
        for (index, (effects, modifiers)) in scenarios.into_iter().enumerate() {
            let mut fields = vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "active_effects".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: effects,
                    },
                ),
            ];
            if let Some(modifiers) = modifiers {
                fields.push((
                    "attributes".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:movement_speed".into())),
                            ("base".into(), Tag::Double(f64::from(0.1_f32))),
                            (
                                "modifiers".into(),
                                Tag::List {
                                    element_type: TagType::Compound,
                                    values: modifiers,
                                },
                            ),
                        ])],
                    },
                ));
            }
            let doc = NamedTag {
                name: "".into(),
                value: Tag::Compound(fields),
            };
            std::fs::write(
                temp.path().join(format!("{index}.nbt")),
                native_persistence::nbt::write(&doc).unwrap(),
            )
            .unwrap();
            documents.push(doc);
        }
        for (walk_speed, old_base) in [(0.25_f32, Some(0.7_f64)), (0.0, Some(0.3)), (0.375, None)] {
            let mut fields = vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "abilities".into(),
                    Tag::Compound(vec![("walkSpeed".into(), Tag::Float(walk_speed))]),
                ),
            ];
            if let Some(base) = old_base {
                fields.push((
                    "attributes".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:movement_speed".into())),
                            ("base".into(), Tag::Double(base)),
                        ])],
                    },
                ));
            }
            let doc = NamedTag {
                name: "".into(),
                value: Tag::Compound(fields),
            };
            std::fs::write(
                temp.path().join(format!("{}.nbt", documents.len())),
                native_persistence::nbt::write(&doc).unwrap(),
            )
            .unwrap();
            documents.push(doc);
        }
        let source = temp.path().join("MovementEffectSaveOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/MovementEffectSaveOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "Java oracle failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = temp.path().join("results.json");
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("MovementEffectSaveOracle")
            .arg(temp.path())
            .arg(documents.len().to_string())
            .arg(&output));
        let results: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        for (index, original) in documents.iter().enumerate() {
            let mut session = session();
            session
                .restore_saved_walking_speed(
                    crate::player_data::read_saved_walking_speed(original).unwrap(),
                )
                .unwrap();
            session.fall.configure(
                crate::player_data::SavedAttribute::with_base(3.0),
                crate::player_data::SavedAttribute::with_base(1.0),
                crate::player_data::read_saved_fall_effects(original).unwrap(),
                Arc::new(OxygenEnchantments::default()),
            );
            session
                .set_saved_movement_speed(
                    crate::player_data::read_loaded_movement_speed_attribute(original).unwrap(),
                )
                .unwrap();
            for tick in 0..6 {
                session.update(TICK);
                let draft = session.draft_player_state(original).unwrap();
                let attribute =
                    crate::player_data::read_saved_movement_speed_attribute(draft.document())
                        .unwrap();
                let java = &results[index][tick];
                assert_eq!(
                    attribute
                        .movement_speed(None, None, false)
                        .unwrap()
                        .to_bits(),
                    (java["value"].as_f64().unwrap() as f32).to_bits(),
                    "case {index} tick {tick}"
                );
                for name in ["speed", "slowness"] {
                    let id = ResourceId::parse(&format!("minecraft:effect.{name}")).unwrap();
                    assert_eq!(
                        attribute.modifier(&id).map(|modifier| modifier.amount),
                        java[name].as_f64(),
                        "case {index} tick {tick}, {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn movement_speed_refreshes_when_equipment_changes() {
        use native_persistence::nbt::{Tag, TagType};
        let mut session = session();
        session
            .set_saved_movement_speed(crate::player_data::SavedAttribute::with_base(0.2))
            .unwrap();
        let component = Tag::Compound(vec![(
            "minecraft:attribute_modifiers".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(vec![
                    (
                        "type".into(),
                        Tag::String("minecraft:movement_speed".into()),
                    ),
                    ("id".into(), Tag::String("example:boots".into())),
                    ("operation".into(), Tag::String("add_value".into())),
                    ("amount".into(), Tag::Double(0.3)),
                    ("slot".into(), Tag::String("feet".into())),
                ])],
            },
        )]);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            crate::inventory::PlayerSlot::Armor(3),
            ItemStack::unresolved("minecraft:leather_boots".into(), 1, component),
        );
        session.set_player_inventory(inventory);
        session.refresh_movement_speed();
        assert_eq!(session.movement_speed(false), 0.5);
        Arc::make_mut(&mut session.inventory)
            .set_slot(crate::inventory::PlayerSlot::Armor(3), None);
        session.refresh_movement_speed();
        assert_eq!(session.movement_speed(false), 0.2_f32);
    }

    #[test]
    fn loaded_walking_speed_overrides_saved_movement_base_and_is_drafted() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let original = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "abilities".into(),
                    Tag::Compound(vec![("walkSpeed".into(), Tag::Float(0.25))]),
                ),
                (
                    "attributes".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:movement_speed".into())),
                            ("base".into(), Tag::Double(0.8)),
                            ("custom".into(), Tag::Int(9)),
                            (
                                "modifiers".into(),
                                Tag::List {
                                    element_type: TagType::Compound,
                                    values: vec![Tag::Compound(vec![
                                        ("id".into(), Tag::String("example:bonus".into())),
                                        ("amount".into(), Tag::Double(0.2)),
                                        (
                                            "operation".into(),
                                            Tag::String("add_multiplied_total".into()),
                                        ),
                                    ])],
                                },
                            ),
                        ])],
                    },
                ),
            ]),
        };
        let before = native_persistence::nbt::write(&original).unwrap();
        let mut session = session();
        session
            .restore_saved_walking_speed(
                crate::player_data::read_saved_walking_speed(&original).unwrap(),
            )
            .unwrap();
        session
            .set_saved_movement_speed(
                crate::player_data::read_loaded_movement_speed_attribute(&original).unwrap(),
            )
            .unwrap();
        session.refresh_movement_speed();
        assert!((session.movement_speed(false) - 0.3).abs() < 1e-7);
        session.update(TICK);
        let draft = session.draft_player_state(&original).unwrap();
        let Tag::List { values, .. } = draft.document().value.get("attributes").unwrap() else {
            panic!()
        };
        assert_eq!(values[0].get("base"), Some(&Tag::Double(0.25)));
        assert_eq!(values[0].get("custom"), Some(&Tag::Int(9)));
        assert_eq!(native_persistence::nbt::write(&original).unwrap(), before);
        assert!(session.restore_saved_walking_speed(0.3).is_err());
    }

    #[test]
    fn jump_factor_uses_feet_then_movement_support() {
        let mut session = session();
        session.motion.position = DVec3::new(8.5, 1.0, 8.5);
        let honey = session.intern_block(state("minecraft:honey_block", &[]));
        assert!(session.set(IVec3::new(8, 0, 8), honey));
        assert_eq!(session.block_jump_factor(), 0.5);
        let stone = session.intern_block(state("minecraft:stone", &[]));
        assert!(session.set(IVec3::new(8, 0, 8), stone));
        assert_eq!(session.block_jump_factor(), 1.0);
        assert!(session.set(IVec3::new(8, 1, 8), honey));
        assert_eq!(session.block_jump_factor(), 0.5);
    }

    #[test]
    fn saved_gravity_and_no_gravity_control_player_motion() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let source = |gravity: f64, disabled: bool| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("NoGravity".into(), Tag::Byte(i8::from(disabled))),
                (
                    "attributes".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:gravity".into())),
                            ("base".into(), Tag::Double(gravity)),
                        ])],
                    },
                ),
            ]),
        };
        for (base, disabled, expected) in
            [(0.2, false, 0.2), (-0.08, false, -0.08), (0.2, true, 0.0)]
        {
            let original = source(base, disabled);
            let mut session = session();
            session.motion.position = DVec3::new(8.5, 10.0, 8.5);
            session.motion.velocity = DVec3::ZERO;
            session.motion.on_ground = false;
            session.player.position = session.motion.position.as_vec3();
            session.player.on_ground = false;
            session
                .restore_saved_gravity(
                    crate::player_data::read_saved_gravity_attribute(&original).unwrap(),
                    crate::player_data::read_saved_no_gravity(&original),
                )
                .unwrap();
            session.update(TICK);
            assert!((session.motion.velocity.y + expected * f64::from(0.98_f32)).abs() < 1e-10);
            assert!(session.draft_player_state(&original).is_ok());
            assert!(
                session
                    .draft_player_state(&source(base + 0.01, disabled))
                    .is_err()
            );
            assert!(
                session
                    .draft_player_state(&source(base, !disabled))
                    .is_err()
            );
            assert!(
                session
                    .restore_saved_gravity(
                        crate::player_data::SavedAttribute::with_base(0.08),
                        false
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn gravity_refreshes_when_equipment_changes() {
        use native_persistence::nbt::{Tag, TagType};
        let mut session = session();
        session
            .restore_saved_gravity(crate::player_data::SavedAttribute::with_base(0.08), false)
            .unwrap();
        let component = Tag::Compound(vec![(
            "minecraft:attribute_modifiers".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(vec![
                    ("type".into(), Tag::String("minecraft:gravity".into())),
                    ("id".into(), Tag::String("example:gravity".into())),
                    ("operation".into(), Tag::String("add_value".into())),
                    ("amount".into(), Tag::Double(0.12)),
                    ("slot".into(), Tag::String("feet".into())),
                ])],
            },
        )]);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            crate::inventory::PlayerSlot::Armor(3),
            ItemStack::unresolved("minecraft:leather_boots".into(), 1, component),
        );
        session.set_player_inventory(inventory);
        session.refresh_gravity();
        assert_eq!(session.gravity, Some(0.2));
        Arc::make_mut(&mut session.inventory)
            .set_slot(crate::inventory::PlayerSlot::Armor(3), None);
        session.refresh_gravity();
        assert_eq!(session.gravity, Some(0.08));
    }

    #[test]
    fn saved_water_efficiency_and_dolphins_grace_affect_swimming() {
        let make = |duration, efficiency| {
            let mut session = session();
            let water = session.intern_block(state("minecraft:water", &[("level", "0")]));
            for x in 7..=9 {
                for y in 1..=3 {
                    for z in 7..=9 {
                        assert!(session.set(IVec3::new(x, y, z), water));
                    }
                }
            }
            session.fluid_ticks.clear();
            session.fluid_pending.clear();
            session.motion.velocity = DVec3::new(0.2, 0.0, 0.0);
            session.motion.on_ground = false;
            session.player.on_ground = false;
            session
                .restore_saved_water_movement_efficiency(
                    crate::player_data::SavedAttribute::with_base(efficiency),
                )
                .unwrap();
            session.fall.configure(
                crate::player_data::SavedAttribute::with_base(3.0),
                crate::player_data::SavedAttribute::with_base(1.0),
                crate::survival::FallEffects {
                    dolphins_grace: Some(crate::survival::EffectDuration {
                        amplifier: 0,
                        duration,
                        hidden: None,
                    }),
                    ..Default::default()
                },
                Arc::new(OxygenEnchantments::default()),
            );
            session.submit_command(PlayerCommand::Move {
                forward: 1.0,
                strafe: 0.0,
                jump: false,
            });
            session.update(TICK);
            session
        };
        let active = make(2, 0.0);
        let expired = make(1, 0.0);
        let efficient = make(1, 1.0);
        assert!((active.motion.velocity.x - 0.2 * f64::from(0.96_f32)).abs() < 1e-7);
        assert!((expired.motion.velocity.x - 0.2 * f64::from(0.8_f32)).abs() < 1e-7);
        assert!(efficient.motion.velocity.z.abs() > expired.motion.velocity.z.abs());
    }

    #[test]
    fn jump_combines_strength_honey_and_effect_in_the_session() {
        let mut session = session();
        let honey = session.intern_block(state("minecraft:honey_block", &[]));
        assert!(session.set(IVec3::new(8, 0, 8), honey));
        session.motion.position = DVec3::new(8.5, 0.9375, 8.5);
        session.player.position = session.motion.position.as_vec3();
        session.motion.on_ground = true;
        session.player.on_ground = true;
        session
            .set_saved_jump_strength(crate::player_data::SavedAttribute::with_base(0.8))
            .unwrap();
        session.fall.configure(
            crate::player_data::SavedAttribute::with_base(3.0),
            crate::player_data::SavedAttribute::with_base(1.0),
            crate::survival::FallEffects {
                jump_boost: Some(crate::survival::EffectDuration {
                    amplifier: 0,
                    duration: 2,
                    hidden: None,
                }),
                ..Default::default()
            },
            Arc::new(OxygenEnchantments::default()),
        );
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: true,
        });
        session.update(TICK);
        let expected = f64::from(0.8_f32 * 0.5_f32 + 0.1_f32);
        assert!((session.motion.position.y - 0.9375 - expected).abs() < 1e-7);
    }

    #[test]
    fn player_bounciness_reflects_from_solid_walls_and_ceilings() {
        for (wall, direction) in [
            (true, DVec3::new(0.5, 0.0, 0.0)),
            (false, DVec3::new(0.0, 0.5, 0.0)),
        ] {
            let mut session = session();
            session
                .set_saved_bounciness(crate::player_data::SavedAttribute::with_base(0.5))
                .unwrap();
            let stone = session.states[3];
            let collision = if wall {
                IVec3::new(5, 2, 4)
            } else {
                IVec3::new(4, 4, 4)
            };
            assert!(session.set(collision, stone));
            session.motion.position = if wall {
                DVec3::new(4.5, 2.2, 4.5)
            } else {
                DVec3::new(4.5, 2.0, 4.5)
            };
            session.motion.velocity = direction;
            session.motion.on_ground = false;
            session.player.position = session.motion.position.as_vec3();
            session.update(TICK);
            let component = if wall {
                session.motion.velocity.x
            } else {
                session.motion.velocity.y
            };
            assert!(component < 0.0, "wall={wall}: {component}");
        }
    }

    #[test]
    fn player_bounciness_applies_to_submerged_collisions() {
        for liquid in ["minecraft:water", "minecraft:lava"] {
            for (block, position, velocity, axis) in [
                (
                    IVec3::new(5, 2, 4),
                    DVec3::new(4.5, 2.2, 4.5),
                    DVec3::X * 0.5,
                    0,
                ),
                (
                    IVec3::new(4, 4, 4),
                    DVec3::new(4.5, 2.0, 4.5),
                    DVec3::Y * 0.5,
                    1,
                ),
                (
                    IVec3::new(4, 1, 4),
                    DVec3::new(4.5, 2.2, 4.5),
                    DVec3::Y * -0.5,
                    2,
                ),
            ] {
                let mut session = liquid_session(liquid);
                session.set_bounce_suppression_tag(Arc::new(BTreeSet::new()));
                session
                    .set_saved_bounciness(crate::player_data::SavedAttribute::with_base(0.5))
                    .unwrap();
                assert!(session.set(block, session.states[3]));
                session.motion.position = position;
                session.motion.velocity = velocity;
                session.motion.on_ground = false;
                session.player.position = position.as_vec3();
                session.update(TICK);
                let result = if axis == 0 {
                    session.motion.velocity.x
                } else {
                    session.motion.velocity.y
                };
                assert!(
                    if axis == 2 {
                        result > 0.0
                    } else {
                        result < 0.0
                    },
                    "{liquid} axis={axis}: {result}"
                );
            }
        }
    }

    #[test]
    fn pick_block_selects_the_target_in_loaded_worlds() {
        let mut session = session();
        session.set_game_mode(GameMode::Creative);
        session.set_player_inventory(PlayerInventory::default());
        assert_eq!(session.ray_hit().unwrap().position, IVec3::new(8, 2, 6));
        session.submit_command(PlayerCommand::PickBlock);
        session.update(TICK);
        let inventory = &session.snapshot().inventory;
        assert_eq!(
            inventory
                .slot(crate::inventory::PlayerSlot::Storage(inventory.selected()))
                .unwrap()
                .id(),
            "minecraft:stone"
        );
    }

    #[test]
    fn player_look_reaches_both_vertical_limits_and_recovers() {
        let mut session = session();
        for (delta, expected) in [
            (2.0, crate::MAX_PITCH),
            (-4.0, -crate::MAX_PITCH),
            (0.1, -crate::MAX_PITCH + 0.1),
        ] {
            session.submit_command(PlayerCommand::Look {
                yaw: 0.0,
                pitch: delta,
            });
            session.update(TICK);
            assert_eq!(session.snapshot().player.pitch, expected);
        }
    }

    #[test]
    fn game_master_blocks_require_creative_and_permission() {
        for mode in [GameMode::Survival, GameMode::Creative] {
            for allowed in [false, true] {
                let mut session = session();
                let position = IVec3::new(8, 2, 6);
                let id = session.intern_block(state("minecraft:command_block", &[]));
                assert!(session.set(position, id));
                session.set_game_mode(mode);
                session.set_game_master_permission(allowed);
                assert_eq!(
                    session.mining_target().is_some(),
                    mode == GameMode::Creative && allowed
                );
                session.submit_command(PlayerCommand::Break);
                session.update(TICK);
                assert_eq!(
                    !matches!(cell(
                        &session.world,
                        session.origin,
                        session.bounds,
                        position,
                        session.states[0]
                    ), Cell::State(actual) if actual == id),
                    mode == GameMode::Creative && allowed
                );
            }
        }
    }

    fn test_heightmap_tags() -> Arc<HeightmapTags> {
        struct Provider;
        impl native_resources::packs::ResourceProvider for Provider {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data_stack(&mut self, _: &str) -> anyhow::Result<Vec<Vec<u8>>> {
                Ok(vec![br#"{"values":["minecraft:stone","minecraft:ice","minecraft:dirt","minecraft:grass_block","minecraft:stone_bricks"]}"#.to_vec()])
            }
        }
        let registry = [
            "air",
            "stone",
            "ice",
            "dirt",
            "grass_block",
            "stone_bricks",
            "water",
        ]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
        Arc::new(HeightmapTags::load(&mut Provider, &registry).unwrap())
    }

    fn saved_heightmap_chunk(
        pos: ChunkPos,
        maps: Option<native_persistence::nbt::Tag>,
    ) -> native_persistence::world::LoadedChunk {
        use native_persistence::nbt::{NamedTag, Tag};
        let mut fields = Vec::new();
        if let Some(maps) = maps {
            fields.push(("Heightmaps".into(), maps));
        }
        native_persistence::world::LoadedChunk {
            position: native_persistence::world::ChunkPos { x: pos.x, z: pos.z },
            data_version: 5023,
            status: Some("minecraft:full".into()),
            document: Arc::new(NamedTag {
                name: "".into(),
                value: Tag::Compound(fields),
            }),
            sections: Vec::new(),
        }
    }

    #[test]
    fn spawn_heightmaps_follow_edits_and_fail_closed_for_unknown_states() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let origin = session.origin;
        session
            .set_spawn_heightmap_tags(test_heightmap_tags())
            .unwrap();
        session
            .prepare_initial_spawn_heightmaps(&saved_heightmap_chunk(home, None))
            .unwrap();
        let at = |session: &LoadedSession, x, z| {
            session
                .spawn_column_heights(origin.x + x, origin.z + z)
                .unwrap()
        };
        assert_eq!(
            at(&session, 8, 6),
            crate::world_spawn::SpawnColumnHeights {
                motion_blocking: 2,
                world_surface: 2,
                ocean_floor: 2,
            }
        );
        assert_eq!(
            session
                .spawn_column_candidate(origin.x + 8, origin.z + 6, None)
                .unwrap(),
            Some(IVec3::new(origin.x + 8, 3, origin.z + 6))
        );
        assert_eq!(
            session
                .spawn_column_candidate(origin.x + 8, origin.z + 6, Some(0))
                .unwrap(),
            Some(IVec3::new(origin.x + 8, 1, origin.z + 6))
        );
        assert!(session.set(IVec3::new(8, 2, 6), session.states[0]));
        assert_eq!(at(&session, 8, 6).motion_blocking, 0);
        let water = session.intern_block(state("minecraft:water", &[("level", "0")]));
        assert!(session.set(IVec3::new(8, 3, 6), water));
        assert_eq!(
            at(&session, 8, 6),
            crate::world_spawn::SpawnColumnHeights {
                motion_blocking: 3,
                world_surface: 3,
                ocean_floor: 0,
            }
        );
        assert_eq!(
            session
                .spawn_column_candidate(origin.x + 8, origin.z + 6, None)
                .unwrap(),
            None
        );
        let invalid = session.intern_block(state("minecraft:water", &[("level", "30")]));
        assert!(session.set(IVec3::new(8, 4, 6), invalid));
        assert!(
            session
                .spawn_column_heights(origin.x + 8, origin.z + 6)
                .is_err()
        );
        assert!(!session.take_stream_errors().is_empty());
    }

    #[test]
    fn spawn_heightmaps_preserve_saved_stale_values_during_edits() {
        use native_persistence::nbt::Tag;
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let origin = session.origin;
        session
            .set_spawn_heightmap_tags(test_heightmap_tags())
            .unwrap();
        // With a 32-block dimension, each 6-bit entry stores first-available Y.
        let mut raw = vec![0i64; 26];
        for column in 0..256 {
            raw[column / 10] |= (30u64 << ((column % 10) * 6)) as i64;
        }
        let maps = Tag::Compound(
            ["MOTION_BLOCKING", "WORLD_SURFACE", "OCEAN_FLOOR"]
                .into_iter()
                .map(|name| (name.into(), Tag::LongArray(raw.clone())))
                .collect(),
        );
        session
            .prepare_initial_spawn_heightmaps(&saved_heightmap_chunk(home, Some(maps)))
            .unwrap();
        assert_eq!(
            session
                .spawn_column_heights(origin.x + 8, origin.z + 6)
                .unwrap()
                .world_surface,
            29
        );
        assert!(session.set(IVec3::new(8, 2, 6), session.states[0]));
        assert_eq!(
            session
                .spawn_column_heights(origin.x + 8, origin.z + 6)
                .unwrap()
                .world_surface,
            29
        );
        assert!(session.set(IVec3::new(8, 30, 6), session.states[3]));
        assert_eq!(
            session
                .spawn_column_heights(origin.x + 8, origin.z + 6)
                .unwrap()
                .world_surface,
            30
        );
        assert!(session.set(IVec3::new(8, 30, 6), session.states[0]));
        assert_eq!(
            session
                .spawn_column_heights(origin.x + 8, origin.z + 6)
                .unwrap()
                .world_surface,
            0
        );
    }

    #[test]
    fn streamed_spawn_heightmaps_follow_their_chunks_across_eviction() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let neighbor = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let origin = session.origin;
        let tags = test_heightmap_tags();
        session.set_spawn_heightmap_tags(Arc::clone(&tags)).unwrap();
        session
            .prepare_initial_spawn_heightmaps(&saved_heightmap_chunk(home, None))
            .unwrap();
        let (mut incoming, _, _) = fixture();
        let mut neighbor_chunk = (*incoming.chunks.remove(&home).unwrap()).clone();
        neighbor_chunk.position = neighbor;
        incoming.chunks.insert(neighbor, Arc::new(neighbor_chunk));
        let maps = crate::spawn_heightmaps::prepare(
            &saved_heightmap_chunk(neighbor, None),
            &incoming,
            HeightBounds {
                min_y: 0,
                max_y: 32,
            },
            &tags,
        )
        .unwrap();
        session.queue_chunk_window(home, 1);
        session.queue_loaded_chunk_with_data(
            neighbor,
            incoming,
            SavedEntityData::Empty,
            Some(maps),
        );
        assert!(
            session
                .spawn_column_heights(origin.x + 24, origin.z + 6)
                .is_err()
        );
        session.update(TICK);
        assert_eq!(
            session
                .spawn_column_heights(origin.x + 24, origin.z + 6)
                .unwrap()
                .world_surface,
            2
        );
        assert!(session.set(IVec3::new(8, 2, 6), session.states[0]));
        session.queue_chunk_window(neighbor, 0);
        session.update(TICK);
        assert!(!session.world.chunks.contains_key(&home));
        assert_eq!(
            session
                .spawn_column_heights(origin.x + 8, origin.z + 6)
                .unwrap()
                .world_surface,
            0
        );
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        assert_eq!(
            session
                .spawn_column_heights(origin.x + 8, origin.z + 6)
                .unwrap()
                .world_surface,
            0
        );
        assert!(
            session
                .spawn_column_heights(origin.x + 24, origin.z + 6)
                .is_err()
        );
    }

    #[test]
    fn unrecognized_block_does_not_produce_a_guessed_spawn_heightmap() {
        let (mut world, _, _) = fixture();
        let pos = *world.chunks.keys().next().unwrap();
        let unknown = world.intern_state(state("example:unknown", &[]));
        world
            .set_block_id(pos.x * 16, 15, pos.z * 16, unknown)
            .unwrap();
        let result = crate::spawn_heightmaps::prepare(
            &saved_heightmap_chunk(pos, None),
            &world,
            HeightBounds {
                min_y: 0,
                max_y: 32,
            },
            &test_heightmap_tags(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn spawn_heightmaps_use_the_active_data_pack_motion_tag() {
        struct EmptyTags;
        impl native_resources::packs::ResourceProvider for EmptyTags {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data_stack(&mut self, _: &str) -> anyhow::Result<Vec<Vec<u8>>> {
                Ok(vec![br#"{"replace":true,"values":[]}"#.to_vec()])
            }
        }
        let registry = ["air", "stone", "dirt", "grass_block", "stone_bricks"]
            .into_iter()
            .map(|name| format!("minecraft:{name}"))
            .collect();
        let tags = Arc::new(HeightmapTags::load(&mut EmptyTags, &registry).unwrap());
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let origin = session.origin;
        session.set_spawn_heightmap_tags(tags).unwrap();
        session
            .prepare_initial_spawn_heightmaps(&saved_heightmap_chunk(home, None))
            .unwrap();
        assert_eq!(
            session
                .spawn_column_heights(origin.x + 8, origin.z + 6)
                .unwrap(),
            crate::world_spawn::SpawnColumnHeights {
                motion_blocking: -1,
                world_surface: 2,
                ocean_floor: -1,
            }
        );
        assert!(session.set(IVec3::new(8, 2, 6), session.states[0]));
        assert_eq!(
            session
                .spawn_column_heights(origin.x + 8, origin.z + 6)
                .unwrap(),
            crate::world_spawn::SpawnColumnHeights {
                motion_blocking: -1,
                world_surface: 0,
                ocean_floor: -1,
            }
        );
    }

    #[test]
    fn saved_items_are_read_only_and_distinguish_unavailable_chunks() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let mut session = session();
        let home = session.player_chunk().unwrap();
        assert!(session.saved_items_in_chunk(home).unwrap().is_none());
        session
            .set_initial_entity_chunk(home, SavedEntityData::Empty)
            .unwrap();
        assert!(
            session
                .saved_items_in_chunk(home)
                .unwrap()
                .unwrap()
                .is_empty()
        );
        let item = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: vec![Tag::Double(8.5), Tag::Double(3.0), Tag::Double(8.5)],
                },
            ),
            ("Age".into(), Tag::Short(120)),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(3)),
                ]),
            ),
            ("unknown".into(), Tag::Int(123)),
        ]);
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Entities".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![
                        Tag::Compound(vec![("id".into(), Tag::String("minecraft:pig".into()))]),
                        item.clone(),
                    ],
                },
            )]),
        };
        let chunk = Arc::new(native_persistence::world::SavedEntityChunk {
            position: native_persistence::world::ChunkPos {
                x: home.x,
                z: home.z,
            },
            document: Arc::new(document),
        });
        session
            .set_initial_entity_chunk(home, SavedEntityData::Present(Arc::clone(&chunk)))
            .unwrap();
        let items = session.saved_items_in_chunk(home).unwrap().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].uuid, Some([1, 2, 3, 4]));
        assert_eq!(items[0].stack.count(), 3);
        assert_eq!(*items[0].source, item);
        assert_eq!(chunk.entities().len(), 2);
        assert!(
            session.snapshot().dropped_items.is_empty(),
            "reading must not spawn items"
        );
        let duplicate = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Entities".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![item.clone(), item],
                },
            )]),
        };
        session
            .set_initial_entity_chunk(
                home,
                SavedEntityData::Present(Arc::new(native_persistence::world::SavedEntityChunk {
                    position: native_persistence::world::ChunkPos {
                        x: home.x,
                        z: home.z,
                    },
                    document: Arc::new(duplicate),
                })),
            )
            .unwrap();
        assert!(session.saved_items_in_chunk(home).is_err());
    }

    #[test]
    fn saved_experience_orb_is_loaded_once_parked_and_collected_by_count() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let remote = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let absolute = session.origin.as_dvec3() + DVec3::new(1.5, 2.0, 1.5);
        let orb = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:experience_orb".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: absolute.to_array().into_iter().map(Tag::Double).collect(),
                },
            ),
            ("NoGravity".into(), Tag::Byte(1)),
            ("Age".into(), Tag::Short(-5)),
            ("Value".into(), Tag::Short(7)),
            ("Count".into(), Tag::Int(3)),
            ("unknown".into(), Tag::Int(123)),
        ]);
        let item = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![5, 6, 7, 8])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: absolute.to_array().into_iter().map(Tag::Double).collect(),
                },
            ),
            ("NoGravity".into(), Tag::Byte(1)),
            ("PickupDelay".into(), Tag::Short(i16::MAX)),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(2)),
                ]),
            ),
        ]);
        let document = Arc::new(NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![home.x, home.z])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![orb.clone(), item.clone()],
                    },
                ),
            ]),
        });
        let chunk = Arc::new(native_persistence::world::SavedEntityChunk {
            position: native_persistence::world::ChunkPos {
                x: home.x,
                z: home.z,
            },
            document: Arc::clone(&document),
        });
        session
            .set_initial_entity_chunk(home, SavedEntityData::Present(chunk))
            .unwrap();
        session.dirty_chunks.insert(home);
        assert!(
            session
                .draft_existing_entity_chunks(&BTreeSet::from([home]))
                .is_err(),
            "an entity that has not loaded must not be treated as removed"
        );
        session.update(TICK);
        assert_eq!(session.snapshot().experience_orbs.len(), 1);
        assert_eq!(session.snapshot().experience_orbs[0].position, absolute);
        let draft = session.draft_existing_entity_chunk(home).unwrap();
        let Tag::List { values, .. } = draft.document().value.get("Entities").unwrap() else {
            panic!("missing entities");
        };
        let saved = crate::saved_orb::read(&values[0]).unwrap().unwrap();
        assert_eq!((saved.age, saved.count), (-4, 3));
        assert_eq!(saved.source.get("unknown"), Some(&Tag::Int(123)));
        assert!(session.draft_existing_falling_entity_chunk(home).is_err());
        assert!(
            session
                .draft_existing_entity_chunks(&BTreeSet::from([home]))
                .is_ok()
        );
        session.queue_chunk_window(remote, 0);
        session.update(TICK);
        assert!(session.snapshot().experience_orbs.is_empty());
        for _ in 0..3 {
            session.update(TICK);
        }
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        assert_eq!(session.snapshot().experience_orbs.len(), 1);
        assert_eq!(session.snapshot().experience_orbs[0].position, absolute);
        session.motion.position = absolute - session.origin.as_dvec3();
        for _ in 0..5 {
            session.update(TICK);
        }
        assert_eq!(session.experience.total, 21);
        assert!(session.snapshot().experience_orbs.is_empty());
        assert!(session.draft_existing_entity_chunk(home).is_err());
        assert_eq!(
            document.value.get("Entities").unwrap(),
            &Tag::List {
                element_type: TagType::Compound,
                values: vec![orb, item]
            }
        );
        session.queue_chunk_window(remote, 0);
        session.update(TICK);
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        assert!(session.snapshot().experience_orbs.is_empty());
    }

    #[test]
    fn entity_source_survives_clean_terrain_eviction_and_failed_reload() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let remote = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let position = session.origin.as_dvec3() + DVec3::new(3.5, 2.0, 3.5);
        let item = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: position.to_array().into_iter().map(Tag::Double).collect(),
                },
            ),
            ("NoGravity".into(), Tag::Byte(1)),
            ("PickupDelay".into(), Tag::Short(i16::MAX)),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(1)),
                ]),
            ),
        ]);
        let document = Arc::new(NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![home.x, home.z])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![item],
                    },
                ),
            ]),
        });
        session
            .set_initial_entity_chunk(
                home,
                SavedEntityData::Present(Arc::new(native_persistence::world::SavedEntityChunk {
                    position: native_persistence::world::ChunkPos {
                        x: home.x,
                        z: home.z,
                    },
                    document: Arc::clone(&document),
                })),
            )
            .unwrap();
        session.update(TICK);
        assert!(!session.has_unsaved_chunk_changes(home));
        session.queue_chunk_window(remote, 0);
        session.update(TICK);
        assert!(!session.world.chunks.contains_key(&home));
        assert!(!session.parked_chunks.contains_key(&home));
        assert!(
            session
                .draft_existing_entity_chunk(home)
                .unwrap()
                .matches_source(&document)
                .unwrap()
        );

        let (incoming, _, _) = fixture();
        session.queue_chunk_window(home, 0);
        session.queue_loaded_chunk(home, incoming);
        session.update(TICK);
        assert_eq!(session.snapshot().dropped_items.len(), 1);
        assert!(
            session
                .draft_existing_entity_chunk(home)
                .unwrap()
                .matches_source(&document)
                .unwrap()
        );
    }

    #[test]
    fn occupied_empty_entity_chunk_survives_terrain_eviction() {
        use native_persistence::nbt::{Tag, TagType};
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let remote = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        session
            .set_initial_entity_chunk(home, SavedEntityData::Empty)
            .unwrap();
        let position = session.origin.as_dvec3() + DVec3::new(3.5, 2.0, 3.5);
        let item = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: position.to_array().into_iter().map(Tag::Double).collect(),
                },
            ),
            ("NoGravity".into(), Tag::Byte(1)),
            ("PickupDelay".into(), Tag::Short(i16::MAX)),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(1)),
                ]),
            ),
        ]);
        session
            .dropped_items
            .spawn_saved(crate::saved_item::read(&item).unwrap().unwrap());
        session.update(TICK);
        session.queue_chunk_window(remote, 0);
        session.update(TICK);
        assert!(!session.world.chunks.contains_key(&home));
        assert!(matches!(
            session.saved_entity_chunk(home),
            Some(SavedEntityData::Empty)
        ));

        let mut vacant = self::session();
        vacant
            .set_initial_entity_chunk(home, SavedEntityData::Empty)
            .unwrap();
        vacant.update(TICK);
        vacant.queue_chunk_window(remote, 0);
        vacant.update(TICK);
        assert!(vacant.saved_entity_chunk(home).is_none());
    }

    #[test]
    fn saved_item_survives_eviction_and_is_not_respawned_after_pickup() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let remote = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let absolute = session.origin.as_dvec3() + DVec3::new(3.5, 2.0, 3.5);
        let item = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: vec![
                        Tag::Double(absolute.x),
                        Tag::Double(absolute.y),
                        Tag::Double(absolute.z),
                    ],
                },
            ),
            ("NoGravity".into(), Tag::Byte(1)),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(3)),
                ]),
            ),
        ]);
        let chunk = Arc::new(native_persistence::world::SavedEntityChunk {
            position: native_persistence::world::ChunkPos {
                x: home.x,
                z: home.z,
            },
            document: Arc::new(NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("Position".into(), Tag::IntArray(vec![home.x, home.z])),
                    (
                        "Entities".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![item.clone()],
                        },
                    ),
                ]),
            }),
        });
        session
            .set_initial_entity_chunk(home, SavedEntityData::Present(chunk))
            .unwrap();
        session.dirty_chunks.insert(home);
        assert!(
            session
                .draft_existing_entity_chunks(&BTreeSet::from([home]))
                .is_err()
        );
        session.update(TICK);
        assert_eq!(session.snapshot().dropped_items.len(), 1);
        let initial = session.snapshot().dropped_items[0].clone();
        let item_draft = session
            .draft_saved_item_entity([1, 2, 3, 4])
            .unwrap()
            .unwrap();
        assert!(
            item_draft
                .matches_source(&NamedTag {
                    name: "".into(),
                    value: item
                })
                .unwrap()
        );
        let saved = crate::saved_item::read(&item_draft.document().value)
            .unwrap()
            .unwrap();
        assert_eq!(saved.age, 1);
        assert_eq!(saved.stack.count(), 3);
        assert!(session.draft_existing_entity_chunk(home).is_ok());
        session.queue_chunk_window(remote, 0);
        session.update(TICK);
        assert!(session.snapshot().dropped_items.is_empty());
        for _ in 0..3 {
            session.update(TICK);
        }
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        let restored = session.snapshot().dropped_items[0].clone();
        assert_eq!(restored.position, initial.position);
        assert_eq!(
            restored.age, 1,
            "entity tick count restarts when its chunk reloads"
        );
        session.motion.position = absolute - session.origin.as_dvec3();
        session.update(TICK);
        assert!(session.snapshot().dropped_items.is_empty());
        assert!(
            session
                .draft_saved_item_entity([1, 2, 3, 4])
                .unwrap()
                .is_none()
        );
        assert!(session.draft_existing_entity_chunk(home).is_err());
        let drafts = session
            .draft_existing_entity_chunks(&BTreeSet::from([home]))
            .unwrap();
        let Some(Tag::List { values, .. }) = drafts[&home].document().value.get("Entities") else {
            panic!("missing entity list")
        };
        assert!(values.is_empty(), "picked-up saved item must not respawn");
        session.queue_chunk_window(remote, 0);
        session.update(TICK);
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        assert!(session.snapshot().dropped_items.is_empty());
    }

    #[test]
    fn saved_item_merge_is_reflected_in_the_entity_chunk_draft() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let position = session.origin.as_dvec3() + DVec3::new(3.5, 2.0, 3.5);
        let make_item = |uuid: [i32; 4], count: i32| {
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:item".into())),
                ("UUID".into(), Tag::IntArray(uuid.to_vec())),
                (
                    "Pos".into(),
                    Tag::List {
                        element_type: TagType::Double,
                        values: position.to_array().into_iter().map(Tag::Double).collect(),
                    },
                ),
                ("NoGravity".into(), Tag::Byte(1)),
                ("future".into(), Tag::Int(42)),
                (
                    "Item".into(),
                    Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:stone".into())),
                        ("count".into(), Tag::Int(count)),
                    ]),
                ),
            ])
        };
        let document = Arc::new(NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![home.x, home.z])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![make_item([1, 2, 3, 4], 1), make_item([5, 6, 7, 8], 2)],
                    },
                ),
            ]),
        });
        session
            .set_initial_entity_chunk(
                home,
                SavedEntityData::Present(Arc::new(native_persistence::world::SavedEntityChunk {
                    position: native_persistence::world::ChunkPos {
                        x: home.x,
                        z: home.z,
                    },
                    document: Arc::clone(&document),
                })),
            )
            .unwrap();
        for _ in 0..40 {
            session.update(TICK);
        }
        assert_eq!(session.snapshot().dropped_items.len(), 1);
        assert_eq!(session.snapshot().dropped_items[0].stack.count(), 3);
        let drafts = session
            .draft_existing_entity_chunks(&BTreeSet::from([home]))
            .unwrap();
        let Some(Tag::List { values, .. }) = drafts[&home].document().value.get("Entities") else {
            panic!()
        };
        assert_eq!(values.len(), 1);
        let saved = crate::saved_item::read(&values[0]).unwrap().unwrap();
        assert_eq!(saved.stack.count(), 3);
        assert_eq!(saved.source.get("future"), Some(&Tag::Int(42)));
        assert!(drafts[&home].matches_source(&document).unwrap());
    }

    #[test]
    fn saved_falling_sand_restores_and_does_not_duplicate_after_chunk_reload() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let remote = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let position = session.origin.as_dvec3() + DVec3::new(3.5, 5.0, 3.5);
        let entity = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:falling_block".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: position.to_array().into_iter().map(Tag::Double).collect(),
                },
            ),
            (
                "BlockState".into(),
                Tag::String("minecraft:red_sand".into()),
            ),
            ("NoGravity".into(), Tag::Byte(1)),
            ("OnGround".into(), Tag::Byte(1)),
            ("Time".into(), Tag::Int(20)),
            ("DropItem".into(), Tag::Byte(0)),
        ]);
        let item_position = position + DVec3::new(4.0, 0.0, 0.0);
        let item_entity = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![5, 6, 7, 8])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: item_position
                        .to_array()
                        .into_iter()
                        .map(Tag::Double)
                        .collect(),
                },
            ),
            ("NoGravity".into(), Tag::Byte(1)),
            ("PickupDelay".into(), Tag::Short(i16::MAX)),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(2)),
                ]),
            ),
        ]);
        let pig = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:pig".into())),
            ("UUID".into(), Tag::IntArray(vec![9, 10, 11, 12])),
            ("unknown".into(), Tag::Long(123)),
        ]);
        let chunk = Arc::new(native_persistence::world::SavedEntityChunk {
            position: native_persistence::world::ChunkPos {
                x: home.x,
                z: home.z,
            },
            document: Arc::new(NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("Position".into(), Tag::IntArray(vec![home.x, home.z])),
                    (
                        "Entities".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![entity.clone(), item_entity, pig.clone()],
                        },
                    ),
                ]),
            }),
        });
        let source_chunk = Arc::clone(&chunk.document);
        session
            .set_initial_entity_chunk(home, SavedEntityData::Present(chunk))
            .unwrap();
        session.dirty_chunks.insert(home);
        session.update(TICK);
        assert_eq!(session.falling_sand.len(), 1);
        assert_eq!(session.falling_sand[0].motion.age, 21);
        assert!(!session.falling_sand[0].drop_item);
        let draft = session
            .draft_saved_falling_entity([1, 2, 3, 4])
            .unwrap()
            .unwrap();
        assert!(
            draft
                .matches_source(&NamedTag {
                    name: "".into(),
                    value: entity.clone()
                })
                .unwrap()
        );
        let saved = crate::saved_falling_block::read(&draft.document().value)
            .unwrap()
            .unwrap();
        assert_eq!(saved.time, 21);
        assert!(!saved.on_ground);
        assert_eq!(saved.position, position);
        let chunk_draft = session.draft_existing_falling_entity_chunk(home).unwrap();
        assert!(chunk_draft.matches_source(&source_chunk).unwrap());
        let Some(Tag::List { values, .. }) = chunk_draft.document().value.get("Entities") else {
            panic!("missing entity list")
        };
        assert_eq!(values[0].get("Time"), Some(&Tag::Int(21)));
        assert_eq!(entity.get("Time"), Some(&Tag::Int(20)));
        let mixed = session.draft_existing_entity_chunk(home).unwrap();
        assert!(mixed.matches_source(&source_chunk).unwrap());
        let Some(Tag::List { values, .. }) = mixed.document().value.get("Entities") else {
            panic!("missing entity list")
        };
        assert_eq!(values[0].get("Time"), Some(&Tag::Int(21)));
        assert_eq!(values[1].get("Age"), Some(&Tag::Short(1)));
        assert_eq!(values[2], pig);

        let remote_document = Arc::new(NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![remote.x, remote.z])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![],
                    },
                ),
            ]),
        });
        session.entity_chunks.insert(
            remote,
            SavedEntityData::Present(Arc::new(native_persistence::world::SavedEntityChunk {
                position: native_persistence::world::ChunkPos {
                    x: remote.x,
                    z: remote.z,
                },
                document: remote_document.clone(),
            })),
        );
        session.falling_sand[0].motion.position.x += 16.0;
        let both = std::collections::BTreeSet::from([home, remote]);
        let drafts = session.draft_existing_entity_chunks(&both).unwrap();
        assert!(drafts[&home].matches_source(&source_chunk).unwrap());
        assert!(drafts[&remote].matches_source(&remote_document).unwrap());
        let Some(Tag::List { values, .. }) = drafts[&remote].document().value.get("Entities")
        else {
            panic!()
        };
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].get("UUID"),
            Some(&Tag::IntArray(vec![1, 2, 3, 4]))
        );
        assert!(
            session
                .draft_existing_entity_chunks(&std::collections::BTreeSet::from([home]))
                .is_err()
        );
        session.entity_chunks.insert(remote, SavedEntityData::Empty);
        let drafts = session.draft_existing_entity_chunks(&both).unwrap();
        assert!(!drafts[&home].source_was_absent());
        assert!(drafts[&remote].source_was_absent());
        assert!(!drafts[&remote].matches_source(&remote_document).unwrap());
        let Some(Tag::List { values, .. }) = drafts[&remote].document().value.get("Entities")
        else {
            panic!()
        };
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].get("UUID"),
            Some(&Tag::IntArray(vec![1, 2, 3, 4]))
        );
        session
            .entity_chunks
            .insert(remote, SavedEntityData::Unavailable);
        assert!(session.draft_existing_entity_chunks(&both).is_err());
        session.falling_sand[0].motion.position.x -= 16.0;
        session.entity_chunks.remove(&remote);

        let restored = session.falling_sand.pop().unwrap();
        assert!(session.draft_existing_falling_entity_chunk(home).is_err());
        let mut moved = restored.clone();
        moved.motion.position.x += 16.0;
        session.falling_sand.push(moved);
        assert!(session.draft_existing_falling_entity_chunk(home).is_err());
        session.falling_sand.clear();
        session.falling_sand.push(restored.clone());
        let mut created = restored;
        created.source = None;
        session.falling_sand.push(created);
        assert!(session.draft_existing_falling_entity_chunk(home).is_err());
        session.falling_sand.pop();
        assert_eq!(
            session.snapshot().falling_blocks[0].kind,
            FallingKind::RedSand
        );
        session.queue_chunk_window(remote, 0);
        session.update(TICK);
        assert!(session.snapshot().falling_blocks.is_empty());
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        assert_eq!(session.falling_sand.len(), 1);
        assert_eq!(session.snapshot().falling_blocks.len(), 1);
        assert_eq!(session.falling_sand[0].motion.age, 22);
    }

    #[test]
    fn restored_sand_respects_saved_drop_and_cancel_flags_on_landing() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        for (drop_item, cancel) in [(false, false), (true, false), (true, true)] {
            let mut session = session();
            let home = session.player_chunk().unwrap();
            let slab = session.intern_block(state(
                "minecraft:oak_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            ));
            assert!(session.set(IVec3::new(10, 1, 8), slab));
            let position = session.origin.as_dvec3() + DVec3::new(10.5, 2.0, 8.5);
            let entity = Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:falling_block".into())),
                (
                    "Pos".into(),
                    Tag::List {
                        element_type: TagType::Double,
                        values: position.to_array().into_iter().map(Tag::Double).collect(),
                    },
                ),
                ("BlockState".into(), Tag::String("minecraft:sand".into())),
                ("OnGround".into(), Tag::Byte(1)),
                ("DropItem".into(), Tag::Byte(i8::from(drop_item))),
                ("CancelDrop".into(), Tag::Byte(i8::from(cancel))),
            ]);
            let chunk = Arc::new(native_persistence::world::SavedEntityChunk {
                position: native_persistence::world::ChunkPos {
                    x: home.x,
                    z: home.z,
                },
                document: Arc::new(NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![(
                        "Entities".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![entity],
                        },
                    )]),
                }),
            });
            session
                .set_initial_entity_chunk(home, SavedEntityData::Present(chunk))
                .unwrap();
            session.activate_saved_falling_sand();
            for _ in 0..20 {
                session.advance_falling_sand();
            }
            assert!(session.falling_sand.is_empty());
            assert_eq!(
                session.dropped_items.snapshot().len(),
                usize::from(drop_item && !cancel)
            );
        }
    }

    #[test]
    fn late_entity_data_is_loaded_without_replacing_resident_blocks() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let original = Arc::clone(session.world.chunks.get(&home).unwrap());
        session
            .set_initial_entity_chunk(home, SavedEntityData::Unavailable)
            .unwrap();
        session.update(TICK);
        let position = session.origin.as_dvec3() + DVec3::new(3.5, 2.0, 3.5);
        let item = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![4, 3, 2, 1])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: [position.x, position.y, position.z]
                        .into_iter()
                        .map(Tag::Double)
                        .collect(),
                },
            ),
            ("NoGravity".into(), Tag::Byte(1)),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(1)),
                ]),
            ),
        ]);
        let entities =
            SavedEntityData::Present(Arc::new(native_persistence::world::SavedEntityChunk {
                position: native_persistence::world::ChunkPos {
                    x: home.x,
                    z: home.z,
                },
                document: Arc::new(NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![(
                        "Entities".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![item],
                        },
                    )]),
                }),
            }));
        session.queue_entity_chunk(home, entities.clone());
        session.update(TICK);
        assert!(Arc::ptr_eq(
            &original,
            session.world.chunks.get(&home).unwrap()
        ));
        assert_eq!(session.snapshot().dropped_items.len(), 1);
        let (incoming, _, _) = fixture();
        session.queue_loaded_chunk_with_entities(home, incoming, entities);
        session.update(TICK);
        assert_eq!(session.snapshot().dropped_items.len(), 1);
    }

    #[test]
    fn spawn_collision_requires_complete_entities_only_when_blocks_are_clear() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let origin = session.origin;
        session
            .set_spawn_heightmap_tags(test_heightmap_tags())
            .unwrap();
        let open = IVec3::new(origin.x + 8, 1, origin.z + 8);
        let solid = IVec3::new(origin.x + 8, 0, origin.z + 8);
        // The known floor decides collision even if the entity region cannot be read.
        assert!(!session.world_spawn_clear(solid).unwrap());
        assert!(session.world_spawn_clear(open).is_err());
        session
            .set_initial_entity_chunk(home, SavedEntityData::Unavailable)
            .unwrap();
        assert!(session.world_spawn_clear(open).is_err());
        session
            .set_initial_entity_chunk(home, SavedEntityData::Empty)
            .unwrap();
        assert!(session.world_spawn_clear(open).unwrap());
        let entity = native_persistence::nbt::Tag::Compound(vec![(
            "id".into(),
            native_persistence::nbt::Tag::String("minecraft:pig".into()),
        )]);
        let document = native_persistence::nbt::NamedTag {
            name: "".into(),
            value: native_persistence::nbt::Tag::Compound(vec![(
                "Entities".into(),
                native_persistence::nbt::Tag::List {
                    element_type: native_persistence::nbt::TagType::Compound,
                    values: vec![entity],
                },
            )]),
        };
        session
            .set_initial_entity_chunk(
                home,
                SavedEntityData::Present(Arc::new(native_persistence::world::SavedEntityChunk {
                    position: native_persistence::world::ChunkPos {
                        x: home.x,
                        z: home.z,
                    },
                    document: Arc::new(document),
                })),
            )
            .unwrap();
        assert!(session.world_spawn_clear(open).is_err());
    }

    #[test]
    fn spawn_collision_requires_neighbor_entities_at_chunk_edges() {
        let (mut world, origin, player) = fixture();
        let home = ChunkPos {
            x: origin.x.div_euclid(16),
            z: origin.z.div_euclid(16),
        };
        let template = world.chunks[&home].as_ref().clone();
        for dx in -1..=0 {
            for dz in -1..=0 {
                let pos = ChunkPos {
                    x: home.x + dx,
                    z: home.z + dz,
                };
                if pos != home {
                    let mut chunk = template.clone();
                    chunk.position = pos;
                    world.chunks.insert(pos, Arc::new(chunk));
                }
            }
        }
        let mut session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session
            .set_spawn_heightmap_tags(test_heightmap_tags())
            .unwrap();
        session
            .set_initial_entity_chunk(home, SavedEntityData::Empty)
            .unwrap();
        // A position far enough inside this chunk does not require neighbors.
        assert!(
            session
                .world_spawn_clear(IVec3::new(origin.x + 2, 1, origin.z + 2))
                .unwrap()
        );
        let corner = IVec3::new(origin.x, 1, origin.z);
        assert!(session.world_spawn_clear(corner).is_err());
        for pos in [
            ChunkPos {
                x: home.x - 1,
                z: home.z,
            },
            ChunkPos {
                x: home.x,
                z: home.z - 1,
            },
        ] {
            session
                .set_initial_entity_chunk(pos, SavedEntityData::Empty)
                .unwrap();
        }
        let diagonal = ChunkPos {
            x: home.x - 1,
            z: home.z - 1,
        };
        session
            .set_initial_entity_chunk(diagonal, SavedEntityData::Unavailable)
            .unwrap();
        assert!(session.world_spawn_clear(corner).is_err());
        session
            .set_initial_entity_chunk(diagonal, SavedEntityData::Empty)
            .unwrap();
        assert!(session.world_spawn_clear(corner).unwrap());
    }

    #[test]
    fn loaded_shared_spawn_checks_collision_and_fixes_the_suggested_height() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let origin = session.origin;
        session
            .set_spawn_heightmap_tags(test_heightmap_tags())
            .unwrap();
        session
            .prepare_initial_spawn_heightmaps(&saved_heightmap_chunk(home, None))
            .unwrap();
        session
            .set_initial_entity_chunk(home, SavedEntityData::Empty)
            .unwrap();
        let suggestion = IVec3::new(origin.x + 8, 4, origin.z + 6);
        let candidates =
            || crate::world_spawn::WorldSpawnCandidates::new(suggestion, 0, 100.0, 0).unwrap();
        assert_eq!(
            session
                .find_loaded_world_spawn(suggestion, Some(candidates()), None)
                .unwrap(),
            IVec3::new(suggestion.x, 3, suggestion.z)
        );
        let water = session.intern_block(state("minecraft:water", &[("level", "0")]));
        assert!(session.set(IVec3::new(8, 3, 6), water));
        assert_eq!(
            session
                .find_loaded_world_spawn(suggestion, Some(candidates()), None)
                .unwrap(),
            IVec3::new(suggestion.x, 4, suggestion.z)
        );
        assert_eq!(
            session
                .find_loaded_world_spawn(suggestion, None, None)
                .unwrap(),
            IVec3::new(suggestion.x, 4, suggestion.z)
        );
        session
            .set_initial_entity_chunk(home, SavedEntityData::Unavailable)
            .unwrap();
        assert!(
            session
                .find_loaded_world_spawn(suggestion, Some(candidates()), None)
                .is_err()
        );
    }

    #[test]
    fn world_respawn_handles_absent_and_destroyed_spawns_without_assuming_unknown_data() {
        use native_persistence::world::{
            DimensionId, PlayerRespawnConfig, RespawnData, SavedWorldBorder,
        };
        use native_world::loaded::BIOME_COUNT;

        for saved in ["absent", "destroyed", "unknown"] {
            for immediate in [false, true] {
                let (mut world, origin, player) = fixture();
                let home = ChunkPos {
                    x: origin.x.div_euclid(16),
                    z: origin.z.div_euclid(16),
                };
                let biome = world.intern_biome("minecraft:plains".into());
                let chunk = Arc::make_mut(world.chunks.get_mut(&home).unwrap());
                Arc::make_mut(chunk.sections.get_mut(&0).unwrap()).biomes =
                    Some(Box::new([biome; BIOME_COUNT]));
                let mut session = LoadedSession::new(
                    world,
                    origin,
                    BuildBounds {
                        min_y: 0,
                        max_y: 32,
                    },
                    player,
                )
                .unwrap();
                session
                    .set_spawn_heightmap_tags(test_heightmap_tags())
                    .unwrap();
                session
                    .prepare_initial_spawn_heightmaps(&saved_heightmap_chunk(home, None))
                    .unwrap();
                session
                    .set_initial_entity_chunk(home, SavedEntityData::Empty)
                    .unwrap();
                if saved == "absent" {
                    session.set_saved_respawn_absent().unwrap();
                } else if saved == "destroyed" {
                    session
                        .set_saved_respawn_support(SavedRespawnSupport {
                            config: PlayerRespawnConfig {
                                data: RespawnData {
                                    dimension: DimensionId::parse("overworld").unwrap(),
                                    position: (origin + IVec3::new(8, 1, 8)).to_array(),
                                    yaw: 0.0,
                                    pitch: 0.0,
                                },
                                forced: false,
                            },
                            environment: fixture_respawn_environment(),
                            tags: Arc::new(respawn_tags(false)),
                            border: SavedWorldBorder::default(),
                            max_health: 35.0,
                        })
                        .unwrap();
                }
                session
                    .set_world_respawn_support(WorldRespawnSupport {
                        spawn: RespawnData {
                            dimension: DimensionId::parse("overworld").unwrap(),
                            position: (origin + IVec3::new(8, 5, 8)).to_array(),
                            yaw: 90.0,
                            pitch: 20.0,
                        },
                        border: SavedWorldBorder::default(),
                        max_health: 35.0,
                        radius: 0,
                        adventure: false,
                        ceiling_spawn_height: None,
                    })
                    .unwrap();
                session.set_death_rules(DeathRules {
                    immediate_respawn: immediate,
                    keep_inventory: saved == "destroyed",
                    ..Default::default()
                });
                let experience = crate::experience::Experience {
                    progress: 0.625,
                    level: 23,
                    total: 720,
                };
                session.set_player_experience(experience);
                session.set_player_score(720);
                session.set_player_vitals(crate::survival::PlayerVitals {
                    health: 0.0,
                    ..Default::default()
                });
                session.death_processed = false;
                session.process_death();
                session.update(TICK);
                if saved == "unknown" {
                    assert!(!session.snapshot().death.unwrap().can_respawn);
                    assert!(session.requested_respawn_window().is_none());
                    continue;
                }
                if !immediate {
                    assert!(session.snapshot().death.unwrap().can_respawn, "{saved}");
                    session.submit_command(PlayerCommand::Respawn);
                    session.update(TICK);
                }
                let snapshot = session.snapshot();
                assert!(snapshot.death.is_none(), "{saved}, immediate={immediate}");
                assert_eq!(
                    snapshot.experience,
                    if saved == "destroyed" {
                        experience
                    } else {
                        Default::default()
                    }
                );
                assert_eq!(session.score, if saved == "destroyed" { 720 } else { 0 });
                if saved == "destroyed" {
                    assert_eq!(
                        snapshot.system_messages.as_ref(),
                        &[crate::SystemMessage {
                            kind: crate::SystemMessageKind::MissingRespawnBlock,
                            tick: snapshot.tick,
                        }]
                    );
                } else {
                    assert!(snapshot.system_messages.is_empty());
                }
                assert_eq!(snapshot.vitals.health, 35.0);
                assert_eq!(session.motion.position, DVec3::new(8.5, 1.0, 8.5));
                assert_eq!(session.player.yaw, 270_f32.to_radians());
                assert_eq!(session.player.pitch, (-20_f32).to_radians());
                assert!(session.saved_respawn_absent);
                assert!(session.respawn_support.is_none());
                let messages = Arc::clone(&snapshot.system_messages);
                session.update(TICK);
                assert!(Arc::ptr_eq(&messages, &session.snapshot().system_messages));
            }
        }
    }

    #[test]
    fn world_respawn_uses_border_center_and_adventure_height_fixup() {
        use native_persistence::world::{DimensionId, RespawnData, SavedWorldBorder};
        let mut session = session();
        let origin = session.origin;
        let home = session.player_chunk().unwrap();
        session
            .set_spawn_heightmap_tags(test_heightmap_tags())
            .unwrap();
        session
            .prepare_initial_spawn_heightmaps(&saved_heightmap_chunk(home, None))
            .unwrap();
        session
            .set_initial_entity_chunk(home, SavedEntityData::Empty)
            .unwrap();
        session.set_saved_respawn_absent().unwrap();
        session
            .set_world_respawn_support(WorldRespawnSupport {
                spawn: RespawnData {
                    dimension: DimensionId::parse("overworld").unwrap(),
                    position: (origin + IVec3::new(14, 7, 8)).to_array(),
                    yaw: 0.0,
                    pitch: 0.0,
                },
                border: SavedWorldBorder {
                    center_x: f64::from(origin.x + 8),
                    center_z: f64::from(origin.z + 8),
                    size: 8.0,
                    lerp_target: 8.0,
                    ..Default::default()
                },
                max_health: 20.0,
                radius: 10,
                adventure: true,
                ceiling_spawn_height: None,
            })
            .unwrap();
        session.set_player_vitals(crate::survival::PlayerVitals {
            health: 0.0,
            ..Default::default()
        });
        session.death_processed = false;
        session.process_death();
        session.update(TICK);
        assert!(session.snapshot().death.unwrap().can_respawn);
        session.submit_command(PlayerCommand::Respawn);
        session.update(TICK);
        assert_eq!(session.motion.position, DVec3::new(8.5, 1.0, 8.5));
        assert!(session.snapshot().death.is_none());
    }

    #[test]
    fn moving_border_restarts_at_request_then_finishes_without_losing_it() {
        use native_persistence::{
            nbt::Tag,
            world::{DimensionId, RespawnData, SavedWorldBorder},
        };
        let mut session = session();
        let origin = session.origin;
        let home = session.player_chunk().unwrap();
        session
            .set_spawn_heightmap_tags(test_heightmap_tags())
            .unwrap();
        // A saved, stale heightmap makes the border-center fixup span several
        // ticks even though the actual floor is at y=0.
        let mut raw = vec![0i64; 26];
        for column in 0..256 {
            raw[column / 10] |= (30u64 << ((column % 10) * 6)) as i64;
        }
        let maps = Tag::Compound(
            ["MOTION_BLOCKING", "WORLD_SURFACE", "OCEAN_FLOOR"]
                .into_iter()
                .map(|name| (name.into(), Tag::LongArray(raw.clone())))
                .collect(),
        );
        session
            .prepare_initial_spawn_heightmaps(&saved_heightmap_chunk(home, Some(maps)))
            .unwrap();
        session
            .set_initial_entity_chunk(home, SavedEntityData::Empty)
            .unwrap();
        session.set_saved_respawn_absent().unwrap();
        session
            .set_world_respawn_support(WorldRespawnSupport {
                spawn: RespawnData {
                    dimension: DimensionId::parse("overworld").unwrap(),
                    position: (origin + IVec3::new(13, 31, 8)).to_array(),
                    yaw: 25.0,
                    pitch: 10.0,
                },
                border: SavedWorldBorder {
                    center_x: f64::from(origin.x + 8),
                    center_z: f64::from(origin.z + 8),
                    size: 14.0,
                    lerp_target: 2.0,
                    lerp_time: 8,
                    ..Default::default()
                },
                max_health: 20.0,
                radius: 0,
                adventure: true,
                ceiling_spawn_height: None,
            })
            .unwrap();
        session.set_player_vitals(crate::survival::PlayerVitals {
            health: 0.0,
            ..Default::default()
        });
        session.death_processed = false;
        session.process_death();
        for _ in 0..12 {
            session.update(TICK);
            if session.snapshot().death.unwrap().can_respawn {
                break;
            }
        }
        assert!(session.snapshot().death.unwrap().can_respawn);
        let prefetched = session.world_respawn_search.as_ref().unwrap();
        assert_eq!(prefetched.search.result().unwrap().x, origin.x + 13);
        assert!(prefetched.border_tick < session.tick);
        assert!(session.tick < 8, "the border must still be moving");
        let requested_at = session.tick;
        session.submit_command(PlayerCommand::Respawn);
        session.update(TICK);
        let death = session.snapshot().death.unwrap();
        assert!(death.respawn_pending);
        assert!(!death.can_respawn);
        assert_eq!(session.respawn_request_tick, Some(requested_at));
        assert_eq!(
            session.world_respawn_search.as_ref().unwrap().border_tick,
            requested_at
        );
        assert_eq!(
            session
                .world_respawn_search
                .as_ref()
                .unwrap()
                .search
                .requested_position()
                .x,
            origin.x + 8
        );
        // Repeated clicks while preparing must not restart the accepted request.
        session.submit_command(PlayerCommand::Respawn);
        session.update(TICK);
        assert_eq!(session.respawn_request_tick, Some(requested_at));
        for _ in 0..12 {
            session.update(TICK);
            if session.snapshot().death.is_none() {
                break;
            }
        }
        assert!(session.snapshot().death.is_none());
        assert_eq!(session.motion.position, DVec3::new(8.5, 1.0, 8.5));
        assert!(session.respawn_request_tick.is_none());
    }

    #[test]
    fn immediate_world_respawn_finishes_while_the_border_is_moving() {
        use native_persistence::world::{DimensionId, RespawnData, SavedWorldBorder};
        let mut session = session();
        let origin = session.origin;
        let home = session.player_chunk().unwrap();
        session
            .set_spawn_heightmap_tags(test_heightmap_tags())
            .unwrap();
        session
            .set_initial_entity_chunk(home, SavedEntityData::Empty)
            .unwrap();
        session.set_saved_respawn_absent().unwrap();
        session
            .set_world_respawn_support(WorldRespawnSupport {
                spawn: RespawnData {
                    dimension: DimensionId::parse("overworld").unwrap(),
                    position: (origin + IVec3::new(8, 31, 8)).to_array(),
                    yaw: 0.0,
                    pitch: 0.0,
                },
                border: SavedWorldBorder {
                    center_x: f64::from(origin.x + 8),
                    center_z: f64::from(origin.z + 8),
                    size: 80.0,
                    lerp_target: 40.0,
                    lerp_time: 1000,
                    ..Default::default()
                },
                max_health: 20.0,
                radius: 0,
                adventure: true,
                ceiling_spawn_height: None,
            })
            .unwrap();
        session.set_death_rules(DeathRules {
            immediate_respawn: true,
            ..Default::default()
        });
        session.set_player_vitals(crate::survival::PlayerVitals {
            health: 0.0,
            ..Default::default()
        });
        session.death_processed = false;
        session.process_death();
        session.update(TICK);
        assert!(session.snapshot().death.unwrap().respawn_pending);
        let request_tick = session.respawn_request_tick.unwrap();
        for _ in 0..12 {
            session.update(TICK);
            if session.snapshot().death.is_none() {
                break;
            }
            assert_eq!(session.respawn_request_tick, Some(request_tick));
            assert_eq!(
                session.world_respawn_search.as_ref().unwrap().border_tick,
                request_tick
            );
        }
        assert!(session.snapshot().death.is_none());
        assert!(session.tick < 1000, "the border must still be moving");
        assert_eq!(session.motion.position, DVec3::new(8.5, 1.0, 8.5));
    }

    fn respawn_tags(invalid_air: bool) -> native_resources::respawn_tags::RespawnBlockTags {
        struct Provider {
            invalid_air: bool,
        }
        impl native_resources::packs::ResourceProvider for Provider {
            fn read_asset(&mut self, _path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data_stack(&mut self, path: &str) -> anyhow::Result<Vec<Vec<u8>>> {
                let invalid = path.ends_with("/invalid_spawn_inside.json") && self.invalid_air;
                let json = if invalid {
                    br#"{"values":["minecraft:air"]}"#.as_slice()
                } else {
                    br#"{"values":[]}"#.as_slice()
                };
                Ok(vec![json.to_vec()])
            }
        }
        let registry = [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:grass_block",
            "minecraft:dirt",
            "minecraft:stone_bricks",
            "minecraft:red_bed",
            "minecraft:respawn_anchor",
            "minecraft:barrel",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        native_resources::respawn_tags::RespawnBlockTags::load(
            &mut Provider { invalid_air },
            registry,
        )
        .unwrap()
    }

    fn fixture_border() -> crate::respawn::RespawnBorder {
        crate::respawn::RespawnBorder {
            min_x: -29_999_984.0,
            max_x: 29_999_984.0,
            min_z: -29_999_984.0,
            max_z: 29_999_984.0,
        }
    }

    fn fixture_respawn_environment()
    -> Arc<native_resources::respawn_environment::RespawnEnvironmentRegistry> {
        use native_resources::packs::{ResourceId, ResourceProvider};
        struct Provider;
        impl ResourceProvider for Provider {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(match path {
                    "data/minecraft/dimension_type/overworld.json" => Some(
                        br#"{"attributes":{"minecraft:gameplay/respawn_anchor_works":true}}"#
                            .to_vec(),
                    ),
                    "data/minecraft/worldgen/biome/plains.json" => {
                        Some(br#"{"attributes":{}}"#.to_vec())
                    }
                    _ => None,
                })
            }
            fn list_data(&mut self, folder: &str) -> anyhow::Result<Vec<ResourceId>> {
                Ok(match folder {
                    "worldgen/biome" => vec![ResourceId::parse("minecraft:plains")?],
                    "timeline" => vec![],
                    _ => unreachable!(),
                })
            }
        }
        Arc::new(
            native_resources::respawn_environment::RespawnEnvironmentRegistry::load(
                &mut Provider,
                "minecraft:overworld",
            )
            .unwrap(),
        )
    }

    #[test]
    fn loaded_bed_and_anchor_search_use_real_sections_tags_and_collision_shapes() {
        let (mut world, origin, player) = fixture();
        let head = world.intern_state(state(
            "minecraft:red_bed",
            &[("facing", "north"), ("part", "head"), ("occupied", "false")],
        ));
        let foot = world.intern_state(state(
            "minecraft:red_bed",
            &[("facing", "north"), ("part", "foot"), ("occupied", "false")],
        ));
        let bed = origin + IVec3::new(8, 1, 8);
        world.set_block_id(bed.x, bed.y, bed.z, head).unwrap();
        world.set_block_id(bed.x, bed.y, bed.z + 1, foot).unwrap();
        let session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let result = session
            .bed_stand_up_position(bed, 0.0, &respawn_tags(false), fixture_border())
            .unwrap();
        assert_eq!(
            result,
            Some(DVec3::new(
                f64::from(bed.x) + 1.5,
                1.0,
                f64::from(bed.z) + 0.5
            ))
        );
        assert_eq!(
            session
                .bed_stand_up_position(bed, 0.0, &respawn_tags(true), fixture_border())
                .unwrap(),
            None
        );

        let (mut world, origin, player) = fixture();
        let anchor = world.intern_state(state("minecraft:respawn_anchor", &[("charges", "4")]));
        let at = origin + IVec3::new(8, 1, 8);
        world.set_block_id(at.x, at.y, at.z, anchor).unwrap();
        let session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        assert_eq!(
            session
                .anchor_stand_up_position(at, &respawn_tags(false), fixture_border())
                .unwrap(),
            Some(DVec3::new(
                f64::from(at.x) + 0.5,
                1.0,
                f64::from(at.z) - 0.5
            ))
        );
    }

    #[test]
    fn saved_spawn_preparation_checks_environment_and_does_not_consume_anchor() {
        use native_persistence::world::{DimensionId, PlayerRespawnConfig, RespawnData};
        use native_resources::respawn_environment::{BedCondition, RespawnEnvironment};

        let (mut world, origin, player) = fixture();
        let bed = world.intern_state(state(
            "minecraft:red_bed",
            &[("facing", "north"), ("part", "head"), ("occupied", "false")],
        ));
        let anchor = world.intern_state(state("minecraft:respawn_anchor", &[("charges", "4")]));
        let bed_pos = origin + IVec3::new(8, 1, 8);
        let anchor_pos = origin + IVec3::new(10, 1, 8);
        world
            .set_block_id(bed_pos.x, bed_pos.y, bed_pos.z, bed)
            .unwrap();
        world
            .set_block_id(anchor_pos.x, anchor_pos.y, anchor_pos.z, anchor)
            .unwrap();
        let session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let mut config = PlayerRespawnConfig {
            data: RespawnData {
                dimension: DimensionId::parse("minecraft:overworld").unwrap(),
                position: bed_pos.to_array(),
                yaw: 0.0,
                pitch: 20.0,
            },
            forced: false,
        };
        let tags = respawn_tags(false);
        let mut environment = RespawnEnvironment::default();
        environment.bed.can_set_spawn = BedCondition::WhenDark;
        let mut possible = |_: &BlockState| -> anyhow::Result<bool> {
            anyhow::bail!("unexpected forced-spawn check")
        };
        assert!(
            session
                .resolve_saved_spawn(
                    &config,
                    environment,
                    false,
                    &tags,
                    fixture_border(),
                    &mut possible
                )
                .unwrap()
                .is_none()
        );
        let result = session
            .resolve_saved_spawn(
                &config,
                environment,
                true,
                &tags,
                fixture_border(),
                &mut possible,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            result.position,
            DVec3::new(f64::from(bed_pos.x) + 1.5, 1.0, f64::from(bed_pos.z) + 0.5)
        );
        assert_eq!(result.pitch, 0.0);
        assert_eq!(result.anchor_charge, None);

        config.data.position = anchor_pos.to_array();
        assert!(
            session
                .resolve_saved_spawn(
                    &config,
                    environment,
                    true,
                    &tags,
                    fixture_border(),
                    &mut possible
                )
                .unwrap()
                .is_none()
        );
        environment.anchor_works = true;
        let result = session
            .resolve_saved_spawn(
                &config,
                environment,
                true,
                &tags,
                fixture_border(),
                &mut possible,
            )
            .unwrap()
            .unwrap();
        assert_eq!(result.anchor_charge, Some(3));
        assert_eq!(
            session
                .world
                .state(
                    session
                        .world
                        .block_id(anchor_pos.x, anchor_pos.y, anchor_pos.z)
                        .unwrap()
                )
                .unwrap()
                .properties["charges"],
            "4"
        );

        config.data.dimension = DimensionId::parse("minecraft:the_nether").unwrap();
        assert!(
            session
                .resolve_saved_spawn(
                    &config,
                    environment,
                    true,
                    &tags,
                    fixture_border(),
                    &mut possible
                )
                .is_err()
        );
    }

    #[test]
    fn saved_bed_and_anchor_respawn_restore_player_and_apply_charge_once() {
        use native_persistence::world::{
            DimensionId, PlayerRespawnConfig, RespawnData, SavedWorldBorder,
        };
        use native_world::loaded::BIOME_COUNT;
        for anchor in [false, true] {
            for keep in [false, true] {
                for immediate in [false, true] {
                    let (mut world, origin, player) = fixture();
                    let biome = world.intern_biome("minecraft:plains".into());
                    let pos = origin + IVec3::new(8, 1, 8);
                    let state_id = if anchor {
                        world.intern_state(state("minecraft:respawn_anchor", &[("charges", "4")]))
                    } else {
                        world.intern_state(state(
                            "minecraft:red_bed",
                            &[("facing", "north"), ("part", "head"), ("occupied", "false")],
                        ))
                    };
                    world.set_block_id(pos.x, pos.y, pos.z, state_id).unwrap();
                    let chunk = world
                        .chunks
                        .get_mut(&ChunkPos {
                            x: 1_000_000,
                            z: -1_000_000,
                        })
                        .unwrap();
                    Arc::make_mut(Arc::make_mut(chunk).sections.get_mut(&0).unwrap()).biomes =
                        Some(Box::new([biome; BIOME_COUNT]));
                    let mut session = LoadedSession::new(
                        world,
                        origin,
                        BuildBounds {
                            min_y: 0,
                            max_y: 32,
                        },
                        player,
                    )
                    .unwrap();
                    let mut inventory = PlayerInventory::default();
                    let item = crate::inventory::ItemStack::new(
                        "minecraft:stone".into(),
                        3,
                        64,
                        native_persistence::nbt::Tag::Compound(vec![]),
                    )
                    .unwrap();
                    inventory
                        .set_slot(crate::inventory::PlayerSlot::Storage(4), Some(item.clone()));
                    inventory.select(4);
                    session.set_player_inventory(inventory);
                    session.set_player_score(42);
                    let experience = crate::experience::Experience {
                        progress: 0.5,
                        level: 17,
                        total: 400,
                    };
                    session.set_player_experience(experience);
                    session.set_death_rules(DeathRules {
                        keep_inventory: keep,
                        immediate_respawn: immediate,
                        show_death_messages: true,
                    });
                    session
                        .set_saved_respawn_support(SavedRespawnSupport {
                            config: PlayerRespawnConfig {
                                data: RespawnData {
                                    dimension: DimensionId::parse("overworld").unwrap(),
                                    position: pos.to_array(),
                                    yaw: 0.0,
                                    pitch: 20.0,
                                },
                                forced: false,
                            },
                            environment: fixture_respawn_environment(),
                            tags: Arc::new(respawn_tags(false)),
                            border: SavedWorldBorder::default(),
                            max_health: 35.0,
                        })
                        .unwrap();
                    session.set_player_vitals(crate::survival::PlayerVitals {
                        health: 0.0,
                        food: 3,
                        ..Default::default()
                    });
                    // Simulate a death during this session; an already-dead saved
                    // player has unknown prior drops and is deliberately excluded.
                    session.death_processed = false;
                    session.process_death();
                    assert_eq!(
                        session
                            .experience_orbs
                            .snapshot()
                            .iter()
                            .map(|orb| orb.value)
                            .sum::<i32>(),
                        if keep { 0 } else { 100 }
                    );
                    session.update(TICK);
                    if !immediate {
                        if !anchor && !keep {
                            assert!(session.set(pos - origin, session.states[0]));
                            session.update(TICK);
                            assert!(!session.snapshot().death.unwrap().can_respawn);
                            assert!(session.set(pos - origin, state_id));
                            session.update(TICK);
                            assert!(session.snapshot().death.unwrap().can_respawn);
                        }
                        let before = session.snapshot();
                        assert!(before.death.unwrap().can_respawn);
                        session.submit_command(PlayerCommand::Respawn);
                        session.update(TICK);
                        if anchor {
                            assert_eq!(
                                before
                                    .world
                                    .loaded()
                                    .unwrap()
                                    .block(pos.x, pos.y, pos.z)
                                    .unwrap()
                                    .properties["charges"],
                                "4"
                            );
                        }
                    }
                    let after = session.snapshot();
                    assert!(
                        after.death.is_none(),
                        "anchor={anchor} keep={keep} immediate={immediate}"
                    );
                    assert_eq!(after.vitals.health, 35.0);
                    assert_eq!(after.vitals.max_health, 35.0);
                    assert_eq!(after.vitals.food, 20);
                    assert_eq!(after.inventory.selected(), if keep { 4 } else { 0 });
                    assert_eq!(
                        after
                            .inventory
                            .slot(crate::inventory::PlayerSlot::Storage(4)),
                        if keep { Some(&item) } else { None }
                    );
                    assert_eq!(session.score, if keep { 42 } else { 0 });
                    assert_eq!(
                        after.experience,
                        if keep { experience } else { Default::default() }
                    );
                    if anchor {
                        assert_eq!(
                            after
                                .world
                                .loaded()
                                .unwrap()
                                .block(pos.x, pos.y, pos.z)
                                .unwrap()
                                .properties["charges"],
                            "3"
                        );
                    }
                    session.update(TICK);
                    if anchor {
                        assert_eq!(
                            session.world.block(pos.x, pos.y, pos.z).unwrap().properties["charges"],
                            "3"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn distant_saved_spawn_chunks_are_parked_then_activated_without_losing_precision() {
        use native_persistence::world::{
            DimensionId, PlayerRespawnConfig, RespawnData, SavedWorldBorder,
        };
        use native_world::loaded::BIOME_COUNT;
        let home = ChunkPos {
            x: 1_000_000,
            z: -1_000_000,
        };
        let remote = ChunkPos {
            x: -1_000_000,
            z: 1_000_000,
        };
        for anchor in [false, true] {
            let (world, origin, player) = fixture();
            let (mut other, _, _) = fixture();
            let mut chunk = (*other.chunks.remove(&home).unwrap()).clone();
            chunk.position = remote;
            other.chunks.insert(remote, Arc::new(chunk));
            let biome = other.intern_biome("minecraft:plains".into());
            let spawn = IVec3::new(remote.x * 16 + 8, 1, remote.z * 16 + 8);
            let block = if anchor {
                state("minecraft:respawn_anchor", &[("charges", "4")])
            } else {
                state(
                    "minecraft:red_bed",
                    &[("facing", "north"), ("part", "head"), ("occupied", "false")],
                )
            };
            let block_id = other.intern_state(block);
            other
                .set_block_id(spawn.x, spawn.y, spawn.z, block_id)
                .unwrap();
            let chunk = other.chunks.get_mut(&remote).unwrap();
            Arc::make_mut(Arc::make_mut(chunk).sections.get_mut(&0).unwrap()).biomes =
                Some(Box::new([biome; BIOME_COUNT]));
            other.saved_fluid_ticks.insert(
                remote,
                vec![SavedFluidTick {
                    fluid: "minecraft:water".into(),
                    position: [spawn.x + 3, 1, spawn.z],
                    delay: 10,
                    priority: 0,
                }],
            );
            let mut session = LoadedSession::new(
                world,
                origin,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            session
                .set_saved_respawn_support(SavedRespawnSupport {
                    config: PlayerRespawnConfig {
                        data: RespawnData {
                            dimension: DimensionId::parse("overworld").unwrap(),
                            position: spawn.to_array(),
                            yaw: 0.0,
                            pitch: 0.0,
                        },
                        forced: false,
                    },
                    environment: fixture_respawn_environment(),
                    tags: Arc::new(respawn_tags(false)),
                    border: SavedWorldBorder::default(),
                    max_health: 20.0,
                })
                .unwrap();
            session.set_player_vitals(crate::survival::PlayerVitals {
                health: 0.0,
                ..Default::default()
            });
            session.death_processed = false;
            session.process_death();
            assert_eq!(session.requested_respawn_window(), Some((remote, 1)));
            session.queue_chunk_window(home, 1);
            session.queue_respawn_window(Some((remote, 1)));
            session.update(TICK);
            let before = session.snapshot();
            assert!(!before.death.unwrap().can_respawn);
            session.queue_loaded_chunk_with_entities(remote, other, SavedEntityData::Empty);
            session.update(TICK);
            let parked = session.snapshot();
            assert!(parked.death.unwrap().can_respawn);
            assert!(!parked.world.loaded().unwrap().chunks.contains_key(&remote));
            assert!(session.parked_chunks.contains_key(&remote));
            assert!(matches!(
                session.saved_entity_chunk(remote),
                Some(SavedEntityData::Empty)
            ));
            assert!(!session.fluid_chunks_seen.contains(&remote));
            assert_eq!(parked.world_revision, before.world_revision);
            let expected = session.prepare_saved_respawn().unwrap().unwrap().position;
            session.submit_command(PlayerCommand::Respawn);
            session.update(TICK);
            let after = session.snapshot();
            assert!(after.death.is_none());
            assert!(matches!(
                session.saved_entity_chunk(remote),
                Some(SavedEntityData::Empty)
            ));
            assert_eq!(after.origin, IVec3::new(remote.x * 16, 0, remote.z * 16));
            assert_eq!(
                after.player.position.as_dvec3() + after.origin.as_dvec3(),
                expected
            );
            assert!(after.world.loaded().unwrap().chunks.contains_key(&remote));
            assert!(!parked.world.loaded().unwrap().chunks.contains_key(&remote));
            assert!(session.fluid_chunks_seen.contains(&remote));
            if anchor {
                assert_eq!(
                    after
                        .world
                        .loaded()
                        .unwrap()
                        .block(spawn.x, spawn.y, spawn.z)
                        .unwrap()
                        .properties["charges"],
                    "3"
                );
                session.update(TICK);
                assert_eq!(
                    session
                        .world
                        .block(spawn.x, spawn.y, spawn.z)
                        .unwrap()
                        .properties["charges"],
                    "3"
                );
            }
        }
    }

    #[test]
    fn loaded_respawn_search_does_not_skip_unknown_shapes_or_missing_chunks() {
        let (mut world, origin, player) = fixture();
        let bed = world.intern_state(state(
            "minecraft:red_bed",
            &[("facing", "north"), ("part", "head"), ("occupied", "false")],
        ));
        let unknown = world.intern_state(state("minecraft:barrel", &[]));
        let at = origin + IVec3::new(8, 1, 8);
        world.set_block_id(at.x, at.y, at.z, bed).unwrap();
        world.set_block_id(at.x + 2, at.y, at.z, unknown).unwrap();
        let session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let tags = respawn_tags(false);
        assert!(
            session
                .bed_stand_up_position(at, 0.0, &tags, fixture_border())
                .is_err()
        );
        let mut view = LoadedDismountView::new(&session, &tags, fixture_border()).unwrap();
        assert!(
            crate::respawn::find_safe_dismount(&mut view, origin + IVec3::new(0, 1, 8), true)
                .is_err()
        );
    }

    #[test]
    fn dead_saved_player_keeps_ticking_world_and_does_not_accept_respawn_or_input() {
        let mut session = session();
        session.set_player_score(42);
        session.set_player_vitals(crate::survival::PlayerVitals {
            health: 0.0,
            ..Default::default()
        });
        let before = session.snapshot();
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: true,
        });
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::Respawn);
        session.update(TICK);
        let after = session.snapshot();
        assert_eq!(after.tick, before.tick + 1);
        assert_eq!(after.player.position, before.player.position);
        assert_eq!(after.vitals.health, 0.0);
        assert_eq!(after.death.unwrap().score, 42);
        assert!(!after.death.unwrap().can_respawn);
        assert_eq!(after.inventory.as_ref(), before.inventory.as_ref());
    }

    #[test]
    fn lethal_fall_drowning_and_starvation_report_their_cause() {
        let mut falling = session();
        falling.set_player_vitals(crate::survival::PlayerVitals {
            health: 1.0,
            ..Default::default()
        });
        falling.motion.position = DVec3::new(8.5, 9.0, 8.5);
        for _ in 0..40 {
            falling.update(TICK);
            if falling.snapshot().death.is_some() {
                break;
            }
        }
        assert_eq!(
            falling.snapshot().death.unwrap().cause,
            DeathCause::FallHigh
        );
        falling.update(TICK);
        assert_eq!(
            falling.snapshot().camera.eye(0.0),
            falling.snapshot().camera.eye(1.0)
        );

        let mut drowning = session();
        let water = drowning.intern_block(state("minecraft:water", &[("level", "0")]));
        for x in 7..=9 {
            for y in 1..=3 {
                for z in 7..=9 {
                    assert!(drowning.set(IVec3::new(x, y, z), water));
                }
            }
        }
        drowning.set_player_vitals(crate::survival::PlayerVitals {
            health: 1.0,
            air: -19,
            ..Default::default()
        });
        drowning.update(TICK);
        assert_eq!(drowning.snapshot().death.unwrap().cause, DeathCause::Drown);

        let mut starving = session();
        starving.set_survival_rules(crate::survival::SurvivalRules {
            difficulty: native_persistence::world::Difficulty::Hard,
            ..Default::default()
        });
        starving.set_player_vitals(crate::survival::PlayerVitals {
            health: 1.0,
            food: 0,
            saturation: 0.0,
            food_tick_timer: 79,
            ..Default::default()
        });
        starving.update(TICK);
        assert_eq!(starving.snapshot().death.unwrap().cause, DeathCause::Starve);
    }

    #[test]
    fn lethal_loaded_death_honors_keep_inventory_and_message_rules() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        for keep in [false, true] {
            let mut session = session();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new("minecraft:stone".into(), 5, 64, Tag::Compound(vec![])),
            );
            inventory.set_slot(
                PlayerSlot::Storage(10),
                ItemStack::new("minecraft:dirt".into(), 3, 64, Tag::Compound(vec![])),
            );
            inventory.set_slot(
                PlayerSlot::Offhand,
                ItemStack::new("minecraft:apple".into(), 2, 64, Tag::Compound(vec![])),
            );
            session.set_player_inventory(inventory);
            session.set_death_rules(DeathRules {
                keep_inventory: keep,
                show_death_messages: false,
                ..Default::default()
            });
            session.set_player_vitals(crate::survival::PlayerVitals {
                health: 1.0,
                ..Default::default()
            });
            session.motion.position = DVec3::new(8.5, 9.0, 8.5);
            for _ in 0..40 {
                session.update(TICK);
                if session.snapshot().death.is_some() {
                    break;
                }
            }
            let snapshot = session.snapshot();
            assert!(!snapshot.death.unwrap().show_message);
            assert!(!snapshot.death.unwrap().can_respawn);
            assert_eq!(
                snapshot.inventory.slot(PlayerSlot::Storage(0)).is_some(),
                keep
            );
            assert_eq!(snapshot.inventory.slot(PlayerSlot::Offhand).is_some(), keep);
            assert_eq!(snapshot.dropped_items.len(), if keep { 0 } else { 3 });
            if !keep {
                let names: Vec<_> = snapshot
                    .dropped_items
                    .iter()
                    .map(|item| item.stack.id())
                    .collect();
                assert_eq!(
                    names,
                    ["minecraft:stone", "minecraft:dirt", "minecraft:apple"]
                );
                for _ in 0..60 {
                    session.update(TICK);
                }
                assert_eq!(session.snapshot().dropped_items.len(), 3);
                assert!(
                    session
                        .snapshot()
                        .inventory
                        .slot(PlayerSlot::Storage(0))
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn unresolvable_death_enchantment_does_not_remove_any_inventory() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        let mut session = session();
        let mut inventory = PlayerInventory::default();
        let item = ItemStack::new(
            "minecraft:stone".into(),
            1,
            64,
            Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![("example:missing".into(), Tag::Int(1))]),
            )]),
        )
        .unwrap();
        inventory.set_slot(PlayerSlot::Storage(0), Some(item.clone()));
        session.set_player_inventory(inventory);
        session.set_player_vitals(crate::survival::PlayerVitals {
            health: 1.0,
            ..Default::default()
        });
        session.motion.position = DVec3::new(8.5, 9.0, 8.5);
        for _ in 0..40 {
            session.update(TICK);
            if session.snapshot().death.is_some() {
                break;
            }
        }
        let snapshot = session.snapshot();
        assert_eq!(snapshot.inventory.slot(PlayerSlot::Storage(0)), Some(&item));
        assert!(snapshot.death.unwrap().inventory_unresolved);
        assert!(snapshot.dropped_items.is_empty());
        assert!(
            session
                .take_stream_errors()
                .iter()
                .any(|error| error.contains("death inventory preserved"))
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn farmland_trampling_matches_java() {
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
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("FarmlandTrampleOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/FarmlandTrampleOracle.java"),
        )
        .unwrap();
        let output = temp.path().join("results.json");
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java farmland oracle failed:\n{}\n{}",
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
            .arg("FarmlandTrampleOracle")
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 64 * 3 * 6);
        for row in &rows {
            let mut rng = native_core::random::LegacyRandom::new(row["seed"].as_i64().unwrap());
            assert_eq!(
                crate::farmland_trampled_by_player(
                    &mut rng,
                    row["distance"].as_f64().unwrap(),
                    row["height"].as_f64().unwrap() as f32
                ),
                row["changed"].as_bool().unwrap(),
                "{row}"
            );
            assert_eq!(
                i64::from(rng.next_int()),
                row["next"].as_i64().unwrap(),
                "{row}"
            );
        }
    }

    #[test]
    fn creative_flight_collides_with_walls_and_ends_on_landing() {
        let mut session = session();
        session.set_game_mode(GameMode::Creative);
        for y in 7..=14 {
            assert!(session.set(IVec3::new(8, y, 7), session.states[3]));
        }
        session.motion.position = DVec3::new(8.5, 8.0, 8.5);
        session.motion.velocity = DVec3::ZERO;
        session.motion.on_ground = false;
        for jump in [true, false, true] {
            session.submit_command(PlayerCommand::Move {
                forward: 0.0,
                strafe: 0.0,
                jump,
            });
            session.update(TICK);
        }
        assert!(session.flight.flying);
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: false,
        });
        for _ in 0..25 {
            session.update(TICK);
        }
        assert!(session.motion.position.z >= 8.3 - 1.0e-6);
        assert!(session.flight.flying);
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: false,
        });
        session.submit_command(PlayerCommand::Sneak(true));
        for _ in 0..60 {
            session.update(TICK);
            if session.motion.on_ground {
                break;
            }
        }
        assert!(session.motion.on_ground);
        assert!(!session.flight.flying);
    }

    #[test]
    fn creative_flight_uses_saved_bounciness_and_shift_suppresses_it() {
        for sneak in [false, true] {
            for x_axis in [false, true] {
                let mut session = session();
                session.set_game_mode(GameMode::Creative);
                session.flight.flying = true;
                session
                    .set_saved_bounciness(crate::player_data::SavedAttribute::with_base(0.5))
                    .unwrap();
                let collision = if x_axis {
                    IVec3::new(5, 8, 4)
                } else {
                    IVec3::new(4, 8, 5)
                };
                assert!(session.set(collision, session.states[3]));
                session.motion.position = DVec3::new(4.5, 8.2, 4.5);
                session.motion.velocity = if x_axis {
                    DVec3::X * 0.5
                } else {
                    DVec3::Z * 0.5
                };
                session.motion.on_ground = false;
                session.player.position = session.motion.position.as_vec3();
                session.submit_command(PlayerCommand::Sneak(sneak));
                session.update(TICK);
                let velocity = if x_axis {
                    session.motion.velocity.x
                } else {
                    session.motion.velocity.z
                };
                if sneak {
                    assert_eq!(velocity, 0.0, "x_axis={x_axis}");
                } else {
                    assert!(
                        (velocity - (-0.25 * f64::from(0.91_f32))).abs() < 1e-12,
                        "x_axis={x_axis}: {velocity}"
                    );
                }
            }
        }
    }

    #[test]
    fn saved_spectator_flight_speed_is_restored_and_wheel_clamps_it() {
        let mut session = session();
        session.set_game_mode(GameMode::Spectator);
        session.restore_saved_flight_speed(0.75).unwrap();
        session.motion.position = DVec3::new(8.5, 10.0, 8.5);
        session.motion.velocity = DVec3::ZERO;
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: true,
        });
        session.update(TICK);
        assert_eq!(session.motion.position.y, 12.25);
        assert!(session.restore_saved_flight_speed(0.1).is_err());
        // Java also clamps on a horizontal-only scroll, whose vertical steps are zero.
        session.submit_command(PlayerCommand::AdjustFlightSpeed(0));
        session.update(TICK);
        assert_eq!(session.flight_speed, 0.2);
        let mut invalid = self::session();
        assert!(invalid.restore_saved_flight_speed(f32::NAN).is_err());
        assert!(invalid.restore_saved_flight_speed(f32::INFINITY).is_err());
    }

    #[test]
    fn spectator_wheel_changes_flight_speed_before_movement() {
        let mut session = session();
        session.set_game_mode(GameMode::Spectator);
        session.motion.position = DVec3::new(8.5, 10.0, 8.5);
        session.motion.velocity = DVec3::ZERO;
        session.submit_command(PlayerCommand::AdjustFlightSpeed(10));
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: true,
        });
        session.update(TICK);
        let expected_speed = 0.05_f32 + 10.0_f32 * 0.005_f32;
        assert_eq!(session.flight_speed, expected_speed);
        assert!(
            (session.motion.position.y - (10.0 + f64::from(expected_speed * 3.0))).abs() < 1e-12
        );
        session.submit_command(PlayerCommand::AdjustFlightSpeed(-100));
        session.update(TICK);
        assert_eq!(session.flight_speed, 0.0);
        session.submit_command(PlayerCommand::AdjustFlightSpeed(100));
        session.update(TICK);
        assert_eq!(session.flight_speed, 0.2);
    }

    #[test]
    fn spectator_flies_through_blocks_without_falling() {
        let mut session = session();
        session.set_game_mode(GameMode::Spectator);
        for y in 1..=3 {
            assert!(session.set(IVec3::new(8, y, 7), session.states[3]));
        }
        session.motion.position = DVec3::new(8.5, 1.0, 8.5);
        session.motion.velocity = DVec3::ZERO;
        session.motion.on_ground = false;
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: false,
        });
        for _ in 0..20 {
            session.update(TICK);
        }
        assert!(session.motion.position.z < 7.0);
        assert_eq!(session.motion.position.y, 1.0);
        assert_eq!(session.fall_distance, 0.0);
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: true,
        });
        session.update(TICK);
        assert!(session.motion.position.y > 1.0);
        let raised = session.motion.position.y;
        session.motion.velocity.y = 0.0;
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: false,
        });
        session.submit_command(PlayerCommand::Sneak(true));
        session.update(TICK);
        assert!(session.motion.position.y < raised);
    }

    #[test]
    fn landing_on_farmland_tramples_and_lifts_player_at_a_fixed_tick() {
        let prepare = |distance: f64| {
            let mut session = session();
            let farmland = session.intern_block(state("minecraft:farmland", &[("moisture", "7")]));
            assert!(session.set(IVec3::new(8, 1, 8), farmland));
            session.motion.position = DVec3::new(8.5, 2.0, 8.5);
            session.motion.velocity = DVec3::new(0.0, -0.2, 0.0);
            session.motion.on_ground = false;
            session.fall_distance = distance;
            session.level_random = native_core::random::LegacyRandom::new(0);
            session
        };
        let mut ordinary = prepare(2.0);
        let mut varying = prepare(2.0);
        let mut short = prepare(0.1);
        let mut spectator = prepare(2.0);
        spectator.set_game_mode(GameMode::Spectator);
        ordinary.update(TICK);
        for _ in 0..5 {
            varying.update(Duration::from_millis(10));
        }
        short.update(TICK);
        spectator.update(TICK);
        let soil = |session: &LoadedSession| {
            session
                .world
                .block(session.origin.x + 8, 1, session.origin.z + 8)
                .unwrap()
                .name
                .clone()
        };
        assert_eq!(soil(&ordinary), "minecraft:dirt");
        assert_eq!(soil(&varying), "minecraft:dirt");
        assert_eq!(soil(&short), "minecraft:farmland");
        assert_eq!(soil(&spectator), "minecraft:farmland");
        assert_eq!(ordinary.motion.position, varying.motion.position);
        assert_eq!(ordinary.motion.position.y, 2.0);
        assert_eq!(ordinary.fall_distance, 0.0);
    }

    #[test]
    fn falling_counts_the_landing_move_and_water_cancels_the_damage() {
        let prepare = || {
            let mut session = session();
            session.motion.position = DVec3::new(8.5, 9.0, 8.5);
            session.motion.velocity = DVec3::ZERO;
            session.motion.on_ground = false;
            session.set_survival_rules(crate::survival::SurvivalRules {
                natural_health_regeneration: false,
                ..Default::default()
            });
            session
        };
        let mut steady = prepare();
        let mut varying = prepare();
        let mut water = prepare();
        let fluid = water.intern_block(state("minecraft:water", &[("level", "0")]));
        for y in 1..=2 {
            assert!(water.set(IVec3::new(8, y, 8), fluid));
        }
        water.fluid_ticks.clear();
        water.fluid_pending.clear();
        for _ in 0..40 {
            steady.update(TICK);
            for _ in 0..5 {
                varying.update(Duration::from_millis(10));
            }
            water.update(TICK);
            if steady.motion.on_ground {
                break;
            }
        }
        assert!(steady.motion.on_ground);
        assert_eq!(steady.motion.position.y, 1.0);
        assert_eq!(steady.vitals.health, 15.0);
        assert_eq!(steady.vitals.health, varying.vitals.health);
        assert_eq!(steady.motion.position, varying.motion.position);
        assert_eq!(steady.fall_distance, 0.0);
        assert_eq!(water.vitals.health, 20.0);

        let mut shallow = prepare();
        let flowing = shallow.intern_block(state("minecraft:water", &[("level", "7")]));
        assert!(shallow.set(IVec3::new(8, 1, 8), flowing));
        shallow.fluid_ticks.clear();
        shallow.fluid_pending.clear();
        shallow.motion.position = DVec3::new(8.5, 1.3, 8.5);
        shallow.motion.velocity = DVec3::new(0.0, -0.8, 0.0);
        shallow.fall_distance = 8.0;
        shallow.update(TICK);
        assert!(shallow.motion.on_ground);
        assert_eq!(
            shallow.vitals.health, 20.0,
            "entering water on the landing tick must reset the fall"
        );

        let mut unsupported = prepare();
        let block = unsupported.intern_block(state("example:unrecognized", &[]));
        assert!(unsupported.set(IVec3::new(8, 0, 8), block));
        for _ in 0..40 {
            unsupported.update(TICK);
            if unsupported.motion.on_ground {
                break;
            }
        }
        assert!(unsupported.motion.on_ground);
        assert_eq!(
            unsupported.vitals.health, 20.0,
            "an unknown collision barrier must not invent fall behavior"
        );
    }

    #[test]
    fn underwater_air_uses_fixed_ticks_and_bubble_columns_refill_it() {
        let prepare = || {
            let mut session = session();
            let water = session.intern_block(state("minecraft:water", &[("level", "0")]));
            for x in 7..=9 {
                for y in 1..=3 {
                    for z in 7..=9 {
                        assert!(session.set(IVec3::new(x, y, z), water));
                    }
                }
            }
            session.fluid_ticks.clear();
            session.fluid_pending.clear();
            session
        };
        let mut a = prepare();
        let mut b = prepare();
        let original = a.snapshot();
        for _ in 0..40 {
            for session in [&mut a, &mut b] {
                session.motion.position = DVec3::new(8.5, 1.0, 8.5);
                session.motion.velocity = DVec3::ZERO;
            }
            for _ in 0..5 {
                a.update(Duration::from_millis(10));
            }
            b.update(TICK);
            assert_eq!(a.snapshot().vitals.air, b.snapshot().vitals.air);
        }
        assert_eq!(original.vitals.air, 300);
        assert_eq!(a.snapshot().vitals.air, 260);
        assert!(a.snapshot().eyes_in_water);
        let bubble = a.intern_block(state("minecraft:bubble_column", &[("drag", "true")]));
        assert!(a.set(IVec3::new(8, 2, 8), bubble));
        a.motion.position = DVec3::new(8.5, 1.0, 8.5);
        a.motion.velocity = DVec3::ZERO;
        a.update(TICK);
        assert!(a.snapshot().eyes_in_water);
        assert_eq!(a.snapshot().vitals.air, 264);

        let mut unloaded = session();
        unloaded.motion.position = DVec3::new(0.1, 1.0, 8.5);
        unloaded.set_player_vitals(crate::survival::PlayerVitals {
            air: 100,
            ..Default::default()
        });
        unloaded.update(TICK);
        assert_eq!(unloaded.snapshot().vitals.air, 100);

        let mut powered = prepare();
        powered.set_player_vitals(crate::survival::PlayerVitals {
            air: 100,
            ..Default::default()
        });
        powered
            .set_player_breathing(
                crate::survival::BreathingEffects {
                    water_breathing: Some(crate::survival::EffectDuration {
                        amplifier: 0,
                        duration: 1,
                        hidden: Some(Box::new(crate::survival::EffectDuration {
                            amplifier: 0,
                            duration: 3,
                            hidden: None,
                        })),
                    }),
                    ..Default::default()
                },
                0.0,
            )
            .unwrap();
        for expected in [104, 108, 112, 111] {
            powered.motion.position = DVec3::new(8.5, 1.0, 8.5);
            powered.motion.velocity = DVec3::ZERO;
            powered.update(TICK);
            assert_eq!(powered.snapshot().vitals.air, expected);
        }
        let original = native_persistence::nbt::NamedTag {
            name: "".into(),
            value: native_persistence::nbt::Tag::Compound(vec![(
                "active_effects".into(),
                native_persistence::nbt::Tag::List {
                    element_type: native_persistence::nbt::TagType::Compound,
                    values: vec![native_persistence::nbt::Tag::Compound(vec![])],
                },
            )]),
        };
        assert!(powered.draft_player_state(&original).is_err());
    }

    #[test]
    fn player_effect_draft_tracks_ticks_and_rejects_untracked_changes() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let original = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "active_effects".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:water_breathing".into())),
                            ("duration".into(), Tag::Int(2)),
                            (
                                "hidden_effect".into(),
                                Tag::Compound(vec![
                                    ("duration".into(), Tag::Int(5)),
                                    ("show_particles".into(), Tag::Byte(0)),
                                ]),
                            ),
                        ])],
                    },
                ),
            ]),
        };
        let before = native_persistence::nbt::write(&original).unwrap();
        let mut session = session();
        session
            .set_player_breathing(
                crate::player_data::read_saved_breathing_effects(&original).unwrap(),
                0.0,
            )
            .unwrap();
        session.update(TICK);
        session.update(TICK);
        let draft = session.draft_player_state(&original).unwrap();
        let Tag::List { values, .. } = draft.document().value.get("active_effects").unwrap() else {
            panic!()
        };
        assert_eq!(values[0].get("duration"), Some(&Tag::Int(3)));
        assert_eq!(values[0].get("show_particles"), Some(&Tag::Byte(0)));
        assert_eq!(native_persistence::nbt::write(&original).unwrap(), before);
        let other = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![]),
        };
        assert!(session.draft_player_state(&other).is_err());
        session
            .set_player_breathing(crate::survival::BreathingEffects::default(), 0.0)
            .unwrap();
        assert!(session.draft_player_state(&original).is_err());
    }

    #[test]
    fn saved_jump_boost_changes_jump_and_expires_before_movement() {
        let make = |duration| {
            let mut session = session();
            session.motion.on_ground = true;
            session.player.on_ground = true;
            session.fall.configure(
                crate::player_data::SavedAttribute::with_base(3.0),
                crate::player_data::SavedAttribute::with_base(1.0),
                crate::survival::FallEffects {
                    jump_boost: Some(crate::survival::EffectDuration {
                        amplifier: 1,
                        duration,
                        hidden: None,
                    }),
                    ..Default::default()
                },
                Arc::new(OxygenEnchantments::default()),
            );
            session.submit_command(PlayerCommand::Move {
                forward: 0.0,
                strafe: 0.0,
                jump: true,
            });
            session.update(TICK);
            session
        };
        let boosted = make(2);
        let expired = make(1);
        let difference = boosted.motion.position.y - expired.motion.position.y;
        assert!(
            (difference - f64::from(0.2_f32)).abs() < 1e-6,
            "{difference}"
        );
    }

    #[test]
    fn slow_falling_resets_fall_distance_and_expires_before_movement() {
        let make = |duration| {
            let mut session = session();
            session.motion.position = DVec3::new(8.5, 10.0, 8.5);
            session.motion.velocity = DVec3::new(0.0, -0.2, 0.0);
            session.motion.on_ground = false;
            session.player.position = session.motion.position.as_vec3();
            session.player.on_ground = false;
            session.fall_distance = 20.0;
            session.fall.configure(
                crate::player_data::SavedAttribute::with_base(3.0),
                crate::player_data::SavedAttribute::with_base(1.0),
                crate::survival::FallEffects {
                    slow_falling: Some(crate::survival::EffectDuration {
                        amplifier: 0,
                        duration,
                        hidden: None,
                    }),
                    ..Default::default()
                },
                Arc::new(OxygenEnchantments::default()),
            );
            session.update(TICK);
            session
        };
        let active = make(2);
        let expired = make(1);
        assert!((active.fall_distance - f64::from(0.2_f32)).abs() < 1e-7);
        assert!(expired.fall_distance > 20.0);
        assert!((active.motion.velocity.y - (-0.21 * f64::from(0.98_f32))).abs() < 1e-7);
        assert!((expired.motion.velocity.y - (-0.28 * f64::from(0.98_f32))).abs() < 1e-7);
    }

    #[test]
    fn levitation_controls_air_motion_and_resets_accumulated_fall() {
        let make = |duration, slow| {
            let mut session = session();
            session.motion.position = DVec3::new(8.5, 10.0, 8.5);
            session.motion.velocity = DVec3::new(0.0, -0.2, 0.0);
            session.motion.on_ground = false;
            session.player.position = session.motion.position.as_vec3();
            session.player.on_ground = false;
            session.fall_distance = 20.0;
            let effect = |amplifier, duration| {
                Some(crate::survival::EffectDuration {
                    amplifier,
                    duration,
                    hidden: None,
                })
            };
            session.fall.configure(
                crate::player_data::SavedAttribute::with_base(3.0),
                crate::player_data::SavedAttribute::with_base(1.0),
                crate::survival::FallEffects {
                    levitation: effect(1, duration),
                    slow_falling: if slow { effect(0, 10) } else { None },
                    ..Default::default()
                },
                Arc::new(OxygenEnchantments::default()),
            );
            session.update(TICK);
            session
        };
        let active = make(2, false);
        let combined = make(2, true);
        let expired = make(1, false);
        let expected = (-0.2 + (0.1 - (-0.2)) * 0.2) * f64::from(0.98_f32);
        assert!((active.motion.velocity.y - expected).abs() < 1e-9);
        assert_eq!(active.motion.velocity.y, combined.motion.velocity.y);
        assert!((active.fall_distance - f64::from(0.2_f32)).abs() < 1e-7);
        assert!(expired.fall_distance > 20.0);
        assert!((expired.motion.velocity.y - (-0.28 * f64::from(0.98_f32))).abs() < 1e-7);
    }

    #[test]
    fn oxygen_attribute_tracks_equipped_enchantments() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        use native_resources::packs::{ResourceId, ResourceProvider};

        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, _: &str) -> anyhow::Result<Vec<ResourceId>> {
                Ok(vec![ResourceId::parse("minecraft:respiration")?])
            }
            fn read_data(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(Some(br#"{"slots":["head"],"effects":{"minecraft:attributes":[{"id":"minecraft:enchantment.respiration","attribute":"minecraft:oxygen_bonus","operation":"add_value","amount":{"type":"minecraft:linear","base":1.0,"per_level_above_first":1.0}}]}}"#.to_vec()))
            }
        }
        let registry = Arc::new(OxygenEnchantments::load(&mut Data).unwrap());
        let mut session = session();
        let water = session.intern_block(state("minecraft:water", &[("level", "0")]));
        for x in 7..=9 {
            for y in 1..=3 {
                for z in 7..=9 {
                    assert!(session.set(IVec3::new(x, y, z), water));
                }
            }
        }
        session.fluid_ticks.clear();
        session.fluid_pending.clear();
        session.set_player_vitals(crate::survival::PlayerVitals {
            air: 100,
            ..Default::default()
        });
        session.player_random = native_core::random::LegacyRandom::new(1);
        session
            .set_saved_breathing(
                Default::default(),
                crate::player_data::SavedAttribute::with_base(0.0),
                registry,
            )
            .unwrap();
        let components = Tag::Compound(vec![(
            "minecraft:enchantments".into(),
            Tag::Compound(vec![("minecraft:respiration".into(), Tag::Int(3))]),
        )]);
        let helmet = ItemStack::new("minecraft:iron_helmet".into(), 1, 1, components).unwrap();
        Arc::make_mut(&mut session.inventory).set_slot(PlayerSlot::Storage(0), Some(helmet));
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::InventoryQuickMove {
            slot: PlayerSlot::Storage(0),
        });
        session.update(TICK);
        assert!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Armor(0))
                .is_some()
        );
        assert_eq!(session.oxygen_bonus, 3.0);
        assert_eq!(session.snapshot().vitals.air, 99);
        session.motion.position = DVec3::new(8.5, 1.0, 8.5);
        session.motion.velocity = DVec3::ZERO;
        session.update(TICK);
        assert_eq!(session.snapshot().vitals.air, 99);
        session.submit_command(PlayerCommand::InventoryQuickMove {
            slot: PlayerSlot::Armor(0),
        });
        session.motion.position = DVec3::new(8.5, 1.0, 8.5);
        session.motion.velocity = DVec3::ZERO;
        session.update(TICK);
        assert!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Armor(0))
                .is_none()
        );
        assert_eq!(session.oxygen_bonus, 0.0);
        assert_eq!(session.snapshot().vitals.air, 99);
        session.motion.position = DVec3::new(8.5, 1.0, 8.5);
        session.motion.velocity = DVec3::ZERO;
        session.update(TICK);
        assert_eq!(session.snapshot().vitals.air, 98);
        assert!(session.oxygen_valid);
    }

    #[test]
    fn armor_snapshot_tracks_inventory_commands_and_preserves_old_snapshots() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        let mut session = session();
        session
            .set_saved_armor(
                crate::player_data::SavedAttribute::with_base(1.25),
                crate::player_data::SavedAttribute::with_base(0.0),
            )
            .unwrap();
        let boots = ItemStack::new(
            "minecraft:diamond_boots".into(),
            1,
            1,
            Tag::Compound(vec![]),
        )
        .unwrap();
        Arc::make_mut(&mut session.inventory).set_slot(PlayerSlot::Storage(0), Some(boots));
        let before = session.snapshot();
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::InventoryQuickMove {
            slot: PlayerSlot::Storage(0),
        });
        session.update(TICK);
        let equipped = session.snapshot();
        assert_eq!(before.armor.unwrap().armor, 1.25);
        assert_eq!(
            equipped.armor.unwrap(),
            crate::ArmorValues {
                armor: 4.25,
                toughness: 2.0
            }
        );
        assert_eq!(equipped.armor.unwrap().display_value(), 4);
        session.submit_command(PlayerCommand::InventoryQuickMove {
            slot: PlayerSlot::Armor(3),
        });
        session.update(TICK);
        assert_eq!(session.snapshot().armor.unwrap().armor, 1.25);
        assert_eq!(equipped.armor.unwrap().armor, 4.25);
    }

    fn fixture_chunk_document(position: ChunkPos) -> native_persistence::nbt::NamedTag {
        use native_persistence::{
            nbt::{NamedTag, Tag, TagType},
            palette,
        };
        let mut indices = vec![0; BLOCK_COUNT];
        for x in 0..16 {
            for z in 0..16 {
                indices[x + z * 16] = 1;
            }
        }
        indices[8 + 6 * 16 + 2 * 256] = 1;
        let data = palette::encode_indices(2, &indices, 4).unwrap().unwrap();
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("xPos".into(), Tag::Int(position.x)),
                ("zPos".into(), Tag::Int(position.z)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                ("unknown_chunk_data".into(), Tag::Long(42)),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("Y".into(), Tag::Byte(0)),
                            (
                                "block_states".into(),
                                Tag::Compound(vec![
                                    (
                                        "palette".into(),
                                        Tag::List {
                                            element_type: TagType::String,
                                            values: vec![
                                                Tag::String("minecraft:air".into()),
                                                Tag::String("minecraft:stone".into()),
                                            ],
                                        },
                                    ),
                                    ("data".into(), Tag::LongArray(data)),
                                ]),
                            ),
                        ])],
                    },
                ),
            ]),
        }
    }

    #[test]
    fn chunk_draft_tracks_first_and_last_block_states_across_eviction() {
        use native_persistence::{
            edit::{BlockChange, draft_chunk_blocks},
            palette::{self, BlockState},
        };
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let source = fixture_chunk_document(home);
        let source_bytes = native_persistence::nbt::write(&source).unwrap();
        assert!(session.draft_changed_chunk(home, &source).is_err());
        let removed = IVec3::new(8, 2, 6);
        let placed = IVec3::new(5, 2, 5);
        let reverted = IVec3::new(4, 2, 4);
        let air = session.states[0];
        let stone = session.states[3];
        assert!(session.set(removed, air));
        assert!(session.set(removed, stone));
        assert!(session.set(removed, air));
        assert!(session.set(placed, stone));
        assert!(session.set(reverted, stone));
        assert!(session.set(reverted, air));
        let draft = session.draft_changed_chunk(home, &source).unwrap();
        assert_eq!(draft.changed_blocks, 2);
        assert_eq!(session.world.block_edits_in_chunk(home).count(), 2);
        assert!(!session.world.block_edits().any(|edit| {
            edit.position
                == BlockPosition {
                    x: session.origin.x + reverted.x,
                    y: session.origin.y + reverted.y,
                    z: session.origin.z + reverted.z,
                }
        }));
        assert!(draft.matches_source(&source).unwrap());
        assert_eq!(
            draft.document().value.get("unknown_chunk_data"),
            source.value.get("unknown_chunk_data")
        );
        let section = palette::decode_sections(&draft.document().value)
            .unwrap()
            .remove(0)
            .blocks
            .unwrap();
        assert_eq!(section.block(8, 2, 6).unwrap().name, "minecraft:air".into());
        assert_eq!(
            section.block(5, 2, 5).unwrap().name,
            "minecraft:stone".into()
        );
        assert_eq!(section.block(4, 2, 4).unwrap().name, "minecraft:air".into());
        let block = |name: &str| BlockState {
            name: name.into(),
            properties: vec![],
        };
        let stale = draft_chunk_blocks(
            &source,
            native_persistence::world::ChunkPos {
                x: home.x,
                z: home.z,
            },
            &[BlockChange {
                x: session.origin.x + reverted.x,
                y: reverted.y,
                z: session.origin.z + reverted.z,
                expected: block("minecraft:air"),
                replacement: block("minecraft:stone"),
            }],
        )
        .unwrap();
        assert!(!draft.matches_source(stale.document()).unwrap());
        assert!(
            session.draft_changed_chunk(home, stale.document()).is_err(),
            "even a reverted edit must check its source"
        );
        let absolute = session.origin + placed;
        Arc::make_mut(&mut session.world)
            .set_block_id(absolute.x, absolute.y, absolute.z, air)
            .unwrap();
        assert!(session.draft_changed_chunk(home, &source).is_err());
        Arc::make_mut(&mut session.world)
            .set_block_id(absolute.x, absolute.y, absolute.z, stone)
            .unwrap();
        session.queue_chunk_window(
            ChunkPos {
                x: home.x + 1,
                z: home.z,
            },
            0,
        );
        session.update(TICK);
        assert!(!session.world.chunks.contains_key(&home));
        assert!(session.parked_chunks.contains_key(&home));
        let parked = session.draft_changed_chunk(home, &source).unwrap();
        assert_eq!(parked.changed_blocks, 2);
        assert_eq!(
            native_persistence::nbt::write(&source).unwrap(),
            source_bytes
        );
    }

    #[test]
    fn changed_chunk_draft_includes_pending_falling_tick_without_writing_source() {
        use native_persistence::nbt::{self, Tag};
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let source = fixture_chunk_document(home);
        let before = nbt::write(&source).unwrap();
        let position = IVec3::new(5, 2, 5);
        let sand = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(position, sand));
        let absolute = session.origin + position;
        session.insert_fluid_tick(
            FluidTickKey {
                x: absolute.x,
                y: absolute.y,
                z: absolute.z,
                kind: FluidKind::Water,
                source: true,
            },
            5,
            0,
            0,
        );
        let draft = session
            .draft_changed_chunk_with_simulated_ticks(home, &source)
            .unwrap();
        assert_eq!(draft.changed_blocks, 1);
        assert!(draft.matches_source(&source).unwrap());
        let Some(Tag::List { values, .. }) = draft.document().value.get("block_ticks") else {
            panic!("missing ticks")
        };
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].get("i"),
            Some(&Tag::String("minecraft:sand".into()))
        );
        assert_eq!(values[0].get("t"), Some(&Tag::Int(2)));
        let Some(Tag::List { values, .. }) = draft.document().value.get("fluid_ticks") else {
            panic!("missing fluid ticks")
        };
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].get("i"),
            Some(&Tag::String("minecraft:water".into()))
        );
        assert_eq!(values[0].get("t"), Some(&Tag::Int(5)));
        assert_eq!(nbt::write(&source).unwrap(), before);
    }

    #[test]
    fn chunk_draft_refuses_edits_without_original_block_data() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let source = fixture_chunk_document(home);
        let world = Arc::make_mut(&mut session.world);
        let chunk = Arc::make_mut(world.chunks.get_mut(&home).unwrap());
        Arc::make_mut(chunk.sections.get_mut(&0).unwrap()).blocks = None;
        assert!(session.set(IVec3::new(5, 2, 5), session.states[3]));
        assert!(session.draft_changed_chunk(home, &source).is_err());
    }

    #[test]
    fn chunk_draft_accepts_streamed_chunks_with_different_registry_order() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let neighbor = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let mut incoming = World::new("minecraft:overworld");
        let stone = incoming.intern_state(state("minecraft:stone", &[]));
        let air = incoming.intern_state(state("minecraft:air", &[]));
        assert_ne!(stone, session.states[3]);
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        for x in 0..16 {
            for z in 0..16 {
                blocks[x + z * 16] = stone;
            }
        }
        blocks[8 + 6 * 16 + 2 * 256] = stone;
        incoming.chunks.insert(
            neighbor,
            Arc::new(Chunk {
                position: neighbor,
                status: Some("minecraft:full".into()),
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
        session.queue_chunk_window(neighbor, 1);
        session.queue_loaded_chunk(neighbor, incoming);
        session.update(TICK);
        assert_eq!(session.origin.x, neighbor.x * 16);
        assert!(session.set(IVec3::new(8, 2, 6), session.states[0]));
        let source = fixture_chunk_document(neighbor);
        let draft = session.draft_changed_chunk(neighbor, &source).unwrap();
        assert_eq!(draft.changed_blocks, 1);
        assert!(draft.matches_source(&source).unwrap());
    }

    #[test]
    fn session_chunk_draft_uses_data_pack_heightmap_tags_and_rejects_unknown_behavior() {
        use native_persistence::nbt::Tag;
        use native_resources::{heightmap_tags::HeightmapTags, packs::ResourceProvider};
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data_stack(&mut self, path: &str) -> anyhow::Result<Vec<Vec<u8>>> {
                if matches!(
                    path,
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                        | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json"
                ) {
                    Ok(vec![
                        br#"{"values":["minecraft:stone","minecraft:sand"]}"#.to_vec(),
                    ])
                } else {
                    Ok(Vec::new())
                }
            }
        }
        let registry = std::collections::BTreeSet::from([
            "minecraft:air".to_owned(),
            "minecraft:sand".to_owned(),
            "minecraft:stone".to_owned(),
        ]);
        let tags = HeightmapTags::load(&mut Data, &registry).unwrap();
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let source = fixture_chunk_document(home);
        assert!(session.set(IVec3::new(8, 2, 6), session.states[0]));
        let sand = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(IVec3::new(5, 2, 5), sand));
        session.schedule_sand(IVec3::new(5, 2, 5), FallingKind::Sand);
        let fluid_position = session.origin + IVec3::new(6, 2, 5);
        session.schedule_fluid(
            IVec3::new(6, 2, 5),
            FluidState {
                kind: FluidKind::Water,
                amount: 8,
                source: true,
                falling: false,
            },
            5,
        );
        let behavior =
            |state: &native_persistence::palette::BlockState| -> anyhow::Result<(bool, bool)> {
                if state.name.matches("minecraft:air") {
                    Ok((true, false))
                } else if state.name.matches("minecraft:stone")
                    || state.name.matches("minecraft:sand")
                {
                    Ok((false, false))
                } else {
                    anyhow::bail!("unsupported block state")
                }
            };
        let draft = session
            .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(home, &source, &tags, behavior)
            .unwrap();
        let maps = draft.document().value.get("Heightmaps").unwrap();
        for name in [
            "WORLD_SURFACE",
            "OCEAN_FLOOR",
            "MOTION_BLOCKING",
            "MOTION_BLOCKING_NO_LEAVES",
        ] {
            let Tag::LongArray(raw) = maps.get(name).unwrap() else {
                panic!("missing map")
            };
            let value = |x: usize, z: usize| {
                let index = x + z * 16;
                ((raw[index / 10] as u64 >> ((index % 10) * 6)) & 63) as u32
            };
            assert_eq!(value(8, 6), 1);
            assert_eq!(value(5, 5), 3);
        }
        let Tag::List {
            values: block_ticks,
            ..
        } = draft.document().value.get("block_ticks").unwrap()
        else {
            panic!("missing block ticks")
        };
        assert_eq!(block_ticks.len(), 1);
        assert_eq!(
            block_ticks[0].get("i"),
            Some(&Tag::String("minecraft:sand".into()))
        );
        let Tag::List {
            values: fluid_ticks,
            ..
        } = draft.document().value.get("fluid_ticks").unwrap()
        else {
            panic!("missing fluid ticks")
        };
        assert_eq!(fluid_ticks.len(), 1);
        assert_eq!(
            fluid_ticks[0].get("i"),
            Some(&Tag::String("minecraft:water".into()))
        );
        assert_eq!(fluid_ticks[0].get("x"), Some(&Tag::Int(fluid_position.x)));
        assert!(draft.matches_source(&source).unwrap());
        assert!(
            session
                .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
                    home,
                    &source,
                    &tags,
                    |state| {
                        if state.name.matches("minecraft:stone") {
                            anyhow::bail!("unsupported stone semantics")
                        }
                        behavior(state)
                    },
                )
                .is_err()
        );
    }

    #[test]
    fn advancing_a_saved_tick_marks_its_chunk_dirty_without_block_edits() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        session.enqueue_saved_fluids(
            home,
            &[SavedFluidTick {
                fluid: "minecraft:water".into(),
                position: [
                    session.origin.x + 4,
                    session.origin.y + 4,
                    session.origin.z + 4,
                ],
                delay: 5,
                priority: 0,
            }],
        );
        assert!(!session.dirty_chunk_positions().contains(&home));
        session.queue_chunk_window(
            ChunkPos {
                x: home.x + 3,
                z: home.z,
            },
            0,
        );

        session.update(TICK);

        assert!(session.dirty_chunk_positions().contains(&home));
        assert!(!session.block_edits.contains_key(&home));
        assert!(!session.world.chunks.contains_key(&home));
        assert!(session.parked_chunks.contains_key(&home));
        let source = fixture_chunk_document(home);
        let draft = session
            .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
                home,
                &source,
                &test_heightmap_tags(),
                |_| anyhow::bail!("heightmap classification should not run for a tick-only edit"),
            )
            .unwrap();
        assert!(draft.matches_source(&source).unwrap());
        let native_persistence::nbt::Tag::List { values, .. } =
            draft.document().value.get("fluid_ticks").unwrap()
        else {
            panic!("missing fluid ticks")
        };
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].get("t"),
            Some(&native_persistence::nbt::Tag::Int(4))
        );
    }

    #[test]
    fn player_save_draft_uses_completed_ticks_and_absolute_rebased_position() {
        use native_persistence::nbt::{NamedTag, Tag, TagType};
        let (world, origin, mut player) = fixture();
        let (source_yaw, source_pitch) = (31.234_56_f32, -12.345_67_f32);
        player.yaw = (source_yaw + 180.0).to_radians();
        player.pitch = (-source_pitch).to_radians();
        let mut session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let position = DVec3::new(8.500_000_123, 1.0, 8.500_000_456);
        let motion = crate::player_data::SavedPlayerMotion {
            velocity: DVec3::ZERO,
            on_ground: true,
            fall_distance: 0.0,
        };
        session.restore_saved_motion(position, motion).unwrap();
        session.set_player_inventory(PlayerInventory::default());
        let original = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "Dimension".into(),
                    Tag::String("minecraft:overworld".into()),
                ),
                (
                    "Rotation".into(),
                    Tag::List {
                        element_type: TagType::Float,
                        values: vec![Tag::Float(source_yaw), Tag::Float(source_pitch)],
                    },
                ),
                ("custom".into(), Tag::String("keep".into())),
            ]),
        };
        let before = native_persistence::nbt::write(&original).unwrap();
        let initial = session.draft_player_state(&original).unwrap();
        assert_eq!(
            crate::player_data::read_saved_position(initial.document()).unwrap(),
            Some(position + origin.as_dvec3())
        );
        assert_eq!(
            initial.document().value.get("Rotation"),
            original.value.get("Rotation")
        );
        session.submit_command(PlayerCommand::Look {
            yaw: 0.05,
            pitch: -0.02,
        });
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: false,
        });
        assert!(session.draft_player_state(&original).is_err());
        session.update(TICK);
        let home = session.player_chunk().unwrap();
        session.queue_chunk_window(
            ChunkPos {
                x: home.x + 1,
                z: home.z,
            },
            1,
        );
        session.update(TICK);
        assert_ne!(session.origin, origin);
        let draft = session.draft_player_state(&original).unwrap();
        assert!(draft.matches_source(&original).unwrap());
        assert_eq!(
            crate::player_data::read_saved_position(draft.document()).unwrap(),
            Some(session.motion.position + session.origin.as_dvec3())
        );
        assert_eq!(
            crate::player_data::read_saved_motion(draft.document()).unwrap(),
            crate::player_data::SavedPlayerMotion {
                velocity: session.motion.velocity,
                on_ground: session.motion.on_ground,
                fall_distance: session.fall_distance
            }
        );
        assert_eq!(
            draft.document().value.get("Rotation"),
            Some(&Tag::List {
                element_type: TagType::Float,
                values: vec![
                    Tag::Float(session.player.yaw.to_degrees() - 180.0),
                    Tag::Float(-session.player.pitch.to_degrees())
                ]
            })
        );
        assert_eq!(
            draft.document().value.get("custom"),
            original.value.get("custom")
        );
        assert_eq!(native_persistence::nbt::write(&original).unwrap(), before);
    }

    #[test]
    fn saved_world_food_use_is_shared_in_snapshots_and_stops_on_release() {
        use crate::animation::{Hand, ItemUseState};
        use crate::inventory::{ItemStack, PlayerSlot};
        use crate::survival::PlayerVitals;
        use native_persistence::nbt::Tag;
        let mut session = session();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Offhand,
            ItemStack::new("minecraft:apple".into(), 2, 64, Tag::Compound(vec![])),
        );
        session.set_player_inventory(inventory);
        session.set_player_vitals(PlayerVitals {
            food: 10,
            ..Default::default()
        });
        let before = session.snapshot();
        session.submit_command(PlayerCommand::UseHeld(true));
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        let using = session.snapshot();
        assert_eq!(
            using.first_person.item_use,
            Some(ItemUseState {
                hand: Hand::Off,
                remaining_ticks: 31
            })
        );
        assert_eq!(before.first_person.item_use, None);
        session.submit_command(PlayerCommand::StopUse);
        session.update(TICK);
        assert_eq!(session.snapshot().first_person.item_use, None);
        assert_eq!(using.first_person.item_use.unwrap().remaining_ticks, 31);
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Offhand)
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn loaded_session_uses_saved_regeneration_rule_on_fixed_ticks() {
        let mut session = session();
        let vitals = crate::survival::PlayerVitals {
            health: 18.0,
            ..Default::default()
        };
        session.set_player_vitals(vitals);
        session.set_survival_rules(crate::survival::SurvivalRules {
            difficulty: native_persistence::world::Difficulty::Normal,
            natural_health_regeneration: false,
            fall_damage: true,
        });
        for _ in 0..20 {
            session.update(TICK);
        }
        assert_eq!(session.snapshot().vitals.health, 18.0);
        session.set_survival_rules(crate::survival::SurvivalRules::default());
        for _ in 0..10 {
            session.update(TICK);
        }
        assert!(session.snapshot().vitals.health > 18.0);
    }

    #[test]
    fn restores_double_precision_and_saved_momentum_before_first_tick() {
        use crate::player_data::SavedPlayerMotion;
        let mut session = session();
        let old_snapshot = session.snapshot();
        let position = DVec3::new(8.500_000_123_456_79, 3.876543210123, 8.5);
        let saved = SavedPlayerMotion {
            velocity: DVec3::new(0.12, -0.14, 0.07),
            on_ground: false,
            fall_distance: 4.5,
        };
        session.restore_saved_motion(position, saved).unwrap();
        assert_eq!(session.motion.position, position);
        assert_eq!(session.motion.velocity, saved.velocity);
        assert_eq!(session.fall_distance, 4.5);
        assert_eq!(old_snapshot.player.position, Vec3::new(8.5, 1.0, 8.5));
        assert_eq!(session.snapshot().player.position, position.as_vec3());

        session.update(TICK);
        assert!((session.motion.position.x - (position.x + saved.velocity.x)).abs() < 1.0e-12);
        assert!((session.motion.position.y - (position.y + saved.velocity.y)).abs() < 1.0e-12);
        assert!((session.motion.position.z - (position.z + saved.velocity.z)).abs() < 1.0e-12);
        assert!(session.fall_distance > 4.5);
        assert!(session.restore_saved_motion(position, saved).is_err());
    }

    #[test]
    fn restores_saved_ground_state_into_snapshot_and_rejects_invalid_motion() {
        use crate::player_data::SavedPlayerMotion;
        let mut session = session();
        let saved = SavedPlayerMotion {
            velocity: DVec3::new(0.0, 0.02, 0.0),
            on_ground: true,
            fall_distance: 2.0,
        };
        session
            .restore_saved_motion(DVec3::new(8.5, 1.0, 8.5), saved)
            .unwrap();
        assert!(session.motion.on_ground);
        assert!(session.snapshot().player.on_ground);
        assert_eq!(session.snapshot().player.vertical_velocity, 0.4);
        let invalid = SavedPlayerMotion {
            velocity: DVec3::splat(f64::NAN),
            ..saved
        };
        assert!(session.restore_saved_motion(DVec3::ZERO, invalid).is_err());
        assert_eq!(session.motion.position, DVec3::new(8.5, 1.0, 8.5));
    }

    #[test]
    fn saved_food_limits_sprinting_and_creative_bypasses_the_limit() {
        let mut session = session();
        let old = session.snapshot();
        let mut vitals = crate::survival::PlayerVitals {
            food: 6,
            ..Default::default()
        };
        session.set_player_vitals(vitals);
        assert_eq!(old.vitals.food, 20);
        session.submit_command(PlayerCommand::Sprint {
            pressed: true,
            backward_pressed: false,
        });
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: false,
        });
        session.update(TICK);
        assert!(!session.snapshot().sprinting);
        vitals.food = 7;
        session.set_player_vitals(vitals);
        session.update(TICK);
        assert!(session.snapshot().sprinting);
        vitals.food = 6;
        session.set_player_vitals(vitals);
        session.update(TICK);
        assert!(!session.snapshot().sprinting);
        session.set_game_mode(GameMode::Creative);
        session.update(TICK);
        assert!(session.snapshot().sprinting);
    }

    #[test]
    fn loaded_axe_use_updates_only_the_new_snapshot_and_the_used_hand() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        for (mode, hand, initial_damage) in [
            (GameMode::Survival, PlayerSlot::Storage(0), 12),
            (GameMode::Survival, PlayerSlot::Offhand, 58),
            (GameMode::Creative, PlayerSlot::Offhand, 12),
            (GameMode::Adventure, PlayerSlot::Storage(0), 12),
        ] {
            let mut session = session();
            session.set_game_mode(mode);
            session.set_axe_transformers(crate::tests::test_axe_transformer());
            let relative = IVec3::new(8, 2, 6);
            let source = session.intern_block(state("minecraft:poplar_log", &[("axis", "x")]));
            assert!(session.set(relative, source));
            session.update(TICK);
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                hand,
                ItemStack::new(
                    "minecraft:wooden_axe".into(),
                    1,
                    1,
                    Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(initial_damage))]),
                ),
            );
            session.set_player_inventory(inventory);
            let old = session.snapshot();
            let absolute = session.origin + relative;
            session.submit_command(PlayerCommand::Place);
            session.update(TICK);
            let after = session.snapshot();
            let block = |snapshot: &WorldSnapshot| {
                snapshot
                    .world
                    .loaded()
                    .unwrap()
                    .block(absolute.x, absolute.y, absolute.z)
                    .unwrap()
                    .clone()
            };
            assert_eq!(block(&old).name, "minecraft:poplar_log");
            assert_eq!(
                block(&after).name,
                if mode == GameMode::Adventure {
                    "minecraft:poplar_log"
                } else {
                    "minecraft:stripped_poplar_log"
                }
            );
            assert_eq!(
                block(&after).properties.get("axis").map(String::as_str),
                Some("x")
            );
            assert_eq!(
                old.inventory
                    .slot(hand)
                    .unwrap()
                    .components()
                    .get("minecraft:damage")
                    .and_then(Tag::as_java_int),
                Some(initial_damage)
            );
            if mode == GameMode::Survival && initial_damage == 58 {
                assert!(after.inventory.slot(hand).is_none());
            } else {
                assert_eq!(
                    after
                        .inventory
                        .slot(hand)
                        .unwrap()
                        .components()
                        .get("minecraft:damage")
                        .and_then(Tag::as_java_int),
                    Some(initial_damage + i32::from(mode == GameMode::Survival))
                );
            }
        }
    }

    #[test]
    fn loaded_soil_tools_preserve_states_and_check_the_block_above() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        for (kind, item, source, properties, expected, expected_properties) in [
            (
                ToolKind::Shovel,
                "minecraft:wooden_shovel",
                "minecraft:podzol",
                &[("snowy", "false")][..],
                "minecraft:dirt_path",
                &[][..],
            ),
            (
                ToolKind::Hoe,
                "minecraft:wooden_hoe",
                "minecraft:dirt_path",
                &[][..],
                "minecraft:farmland",
                &[("moisture", "0")][..],
            ),
            (
                ToolKind::Hoe,
                "minecraft:wooden_hoe",
                "minecraft:coarse_dirt",
                &[][..],
                "minecraft:dirt",
                &[][..],
            ),
        ] {
            for (blocked, offhand) in [(false, false), (false, true), (true, false)] {
                let mut session = session();
                session.set_tool_transformers(kind, crate::tests::test_soil_transformer(kind));
                let relative = IVec3::new(8, 2, 6);
                let source_id = session.intern_block(state(source, properties));
                assert!(session.set(relative, source_id));
                let above = session.intern_block(state(
                    if blocked {
                        "minecraft:stone"
                    } else {
                        "minecraft:cave_air"
                    },
                    &[],
                ));
                assert!(session.set(relative + IVec3::Y, above));
                session.update(TICK);
                let mut inventory = PlayerInventory::default();
                let hand = if offhand {
                    PlayerSlot::Offhand
                } else {
                    PlayerSlot::Storage(0)
                };
                inventory.set_slot(
                    hand,
                    ItemStack::new(item.into(), 1, 1, Tag::Compound(vec![])),
                );
                session.set_player_inventory(inventory);
                let old = session.snapshot();
                let position = session.origin + relative;
                session.submit_command(PlayerCommand::Place);
                session.update(TICK);
                let after = session.snapshot();
                let block = |snapshot: &WorldSnapshot| {
                    snapshot
                        .world
                        .loaded()
                        .unwrap()
                        .block(position.x, position.y, position.z)
                        .unwrap()
                        .clone()
                };
                assert_eq!(block(&old), state(source, properties));
                assert_eq!(
                    block(&after),
                    if blocked && source == "minecraft:dirt_path" {
                        state("minecraft:dirt", &[])
                    } else if blocked {
                        state(source, properties)
                    } else {
                        state(expected, expected_properties)
                    }
                );
                assert_eq!(
                    after
                        .inventory
                        .slot(hand)
                        .unwrap()
                        .components()
                        .get("minecraft:damage")
                        .and_then(Tag::as_java_int),
                    (!blocked).then_some(1)
                );
            }
        }
    }

    #[test]
    fn loaded_covered_soil_reverts_after_a_tick_and_keeps_older_snapshots() {
        for (name, properties) in [
            ("minecraft:dirt_path", &[][..]),
            ("minecraft:farmland", &[("moisture", "7")][..]),
        ] {
            for remove_cover in [false, true] {
                let mut session = session();
                let relative = IVec3::new(8, 2, 6);
                let id = session.intern_block(state(name, properties));
                assert!(session.set(relative, id));
                session.update(TICK);
                let old = session.snapshot();
                assert!(session.set(relative + IVec3::Y, session.states[3]));
                session.update(TICK);
                let absolute = session.origin + relative;
                let name_at = |snapshot: &WorldSnapshot| {
                    snapshot
                        .world
                        .loaded()
                        .unwrap()
                        .block(absolute.x, absolute.y, absolute.z)
                        .unwrap()
                        .name
                        .clone()
                };
                assert_eq!(name_at(&session.snapshot()), name);
                if remove_cover {
                    assert!(session.set(relative + IVec3::Y, session.states[0]));
                }
                session.update(TICK);
                assert_eq!(
                    name_at(&session.snapshot()),
                    if remove_cover && name == "minecraft:farmland" {
                        name
                    } else {
                        "minecraft:dirt"
                    }
                );
                assert_eq!(name_at(&old), name);
            }
        }
    }

    #[test]
    fn pending_soil_tick_marks_its_chunk_and_is_included_in_tick_draft() {
        for (name, properties) in [
            ("minecraft:dirt_path", &[][..]),
            ("minecraft:farmland", &[("moisture", "7")][..]),
        ] {
            let mut session = session();
            let relative = IVec3::new(8, 2, 6);
            let id = session.intern_block(state(name, properties));
            assert!(session.set(relative, id));
            assert!(session.set(relative + IVec3::Y, session.states[3]));
            let chunk = session.player_chunk().unwrap();
            session.dirty_chunks.clear();
            assert!(session.dirty_chunk_positions().contains(&chunk));
            assert_eq!(
                session.draft_block_ticks(chunk).unwrap(),
                vec![SavedBlockTick {
                    block: name.into(),
                    position: (session.origin + relative).to_array(),
                    delay: 1,
                    priority: 0,
                }]
            );

            session.tick = 1;
            session.advance_soil_ticks();
            assert!(session.draft_block_ticks(chunk).unwrap().is_empty());
        }
    }

    #[test]
    fn soil_tick_lifts_player_items_orbs_and_falling_blocks() {
        use native_persistence::nbt::Tag;
        for (name, properties) in [
            ("minecraft:dirt_path", &[][..]),
            ("minecraft:farmland", &[("moisture", "7")][..]),
        ] {
            let mut session = session();
            let soil = IVec3::new(8, 2, 6);
            let absolute = session.origin + soil;
            let soil_id = session.intern_block(state(name, properties));
            assert!(session.set(soil, soil_id));
            assert!(session.set(soil + IVec3::Y, session.states[3]));
            let relative_position = soil.as_dvec3() + DVec3::new(0.5, 15.0 / 16.0, 0.5);
            let absolute_position = session.origin.as_dvec3() + relative_position;
            session.motion.position = relative_position;
            session.player.position = relative_position.as_vec3();
            session
                .dropped_items
                .spawn_saved(crate::saved_item::SavedItemState {
                    source: Arc::new(Tag::Compound(vec![])),
                    uuid: None,
                    position: absolute_position,
                    velocity: DVec3::ZERO,
                    on_ground: false,
                    no_gravity: true,
                    age: 0,
                    pickup_delay: 10,
                    health: 5,
                    fire_ticks: 0,
                    invulnerable: false,
                    invulnerable_time: 0,
                    owner: None,
                    thrower: None,
                    stack: ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![]))
                        .unwrap(),
                });
            session
                .experience_orbs
                .spawn_saved(crate::saved_orb::SavedOrb {
                    source: Arc::new(Tag::Compound(vec![])),
                    uuid: None,
                    position: absolute_position,
                    velocity: DVec3::ZERO,
                    on_ground: false,
                    no_gravity: true,
                    age: 0,
                    value: 1,
                    count: 1,
                    health: 5,
                    fire_ticks: 0,
                    invulnerable: false,
                    invulnerable_time: 0,
                });
            let motion = crate::falling_block::AirborneMotion {
                position: absolute_position,
                velocity: DVec3::ZERO,
                age: 0,
                no_gravity: true,
            };
            session.falling_sand.push(FallingSand {
                previous: motion.position,
                motion,
                kind: FallingKind::Sand,
                drop_item: true,
                cancel_drop: false,
                on_ground: false,
                source: None,
            });
            let old_items = session.dropped_items.snapshot();
            let old_orbs = session.experience_orbs.snapshot();
            session.tick = 1;
            session.advance_block_ticks();

            assert_eq!(session.motion.position.y, f64::from(soil.y) + 1.0, "{name}");
            assert_eq!(session.player.position.y, soil.y as f32 + 1.0);
            let current_items = session.dropped_items.snapshot();
            let item = &current_items[0];
            assert_eq!(item.position.y, f64::from(absolute.y) + 1.0);
            assert_eq!(item.previous_position, item.position);
            let orb = session.experience_orbs.snapshot()[0];
            assert_eq!(orb.position.y, f64::from(absolute.y) + 1.0);
            assert_eq!(orb.previous_position, orb.position);
            assert_eq!(
                session.falling_sand[0].motion.position.y,
                f64::from(absolute.y) + 1.0
            );
            assert_eq!(
                session.falling_sand[0].previous,
                session.falling_sand[0].motion.position
            );
            assert_eq!(old_items[0].position, absolute_position);
            assert_eq!(old_orbs[0].position, absolute_position);
        }
    }

    #[test]
    fn surviving_farmland_does_not_lift_entities() {
        let mut session = session();
        let soil = IVec3::new(8, 2, 6);
        let farmland = session.intern_block(state("minecraft:farmland", &[("moisture", "7")]));
        assert!(session.set(soil, farmland));
        assert!(session.set(soil + IVec3::Y, session.states[3]));
        assert!(session.set(soil + IVec3::Y, session.states[0]));
        let before = soil.as_dvec3() + DVec3::new(0.5, 15.0 / 16.0, 0.5);
        session.motion.position = before;
        session.player.position = before.as_vec3();
        session.tick = 1;
        session.advance_block_ticks();
        assert_eq!(session.motion.position, before);
        assert!(
            matches!(cell(&session.world, session.origin, session.bounds, soil, session.states[0]), Cell::State(id) if id == farmland)
        );
    }

    #[test]
    fn soil_placement_pushes_before_collision_check_only_for_covered_path() {
        for (block, covered, mode, lifted) in [
            (Block::DirtPath, true, GameMode::Survival, true),
            (Block::DirtPath, false, GameMode::Survival, false),
            (Block::Farmland(0), true, GameMode::Survival, false),
            (Block::DirtPath, true, GameMode::Spectator, false),
        ] {
            let mut session = session();
            session.set_game_mode(mode);
            let target = IVec3::new(8, 2, 7);
            if covered {
                assert!(session.set(target + IVec3::Y, session.states[3]));
            }
            let position = target.as_dvec3() + DVec3::new(0.5, 15.0 / 16.0, 0.5);
            session.motion.position = position;
            session.player.position = position.as_vec3();
            let result = session.placement_block(target, block).unwrap();
            assert_eq!(result, if covered { Block::Dirt } else { block });
            assert_eq!(
                session.motion.position.y,
                position.y + if lifted { 1.0 / 16.0 } else { 0.0 }
            );
            if covered {
                assert_eq!(
                    player_intersects_block(session.player, target, result),
                    !lifted
                );
            }
        }
    }

    #[test]
    fn scheduled_soil_tick_does_not_push_spectator_but_pushes_item() {
        use native_persistence::nbt::Tag;
        let mut session = session();
        session.set_game_mode(GameMode::Spectator);
        let soil = IVec3::new(8, 2, 7);
        let path = session.intern_block(state("minecraft:dirt_path", &[]));
        assert!(session.set(soil, path));
        assert!(session.set(soil + IVec3::Y, session.states[3]));
        let relative = soil.as_dvec3() + DVec3::new(0.5, 15.0 / 16.0, 0.5);
        let absolute = session.origin.as_dvec3() + relative;
        session.motion.position = relative;
        session.player.position = relative.as_vec3();
        session
            .dropped_items
            .spawn_saved(crate::saved_item::SavedItemState {
                source: Arc::new(Tag::Compound(vec![])),
                uuid: None,
                position: absolute,
                velocity: DVec3::ZERO,
                on_ground: false,
                no_gravity: true,
                age: 0,
                pickup_delay: i16::MAX,
                health: 5,
                fire_ticks: 0,
                invulnerable: false,
                invulnerable_time: 0,
                owner: None,
                thrower: None,
                stack: ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![]))
                    .unwrap(),
            });
        session.tick = 1;
        session.advance_block_ticks();
        assert_eq!(session.motion.position, relative);
        assert_eq!(
            session.dropped_items.snapshot()[0].position.y,
            absolute.y + 1.0 / 16.0
        );
    }

    #[test]
    fn placing_covered_soil_moves_items_only_for_dirt_path() {
        use crate::inventory::PlayerSlot;
        use native_persistence::nbt::Tag;
        for (block, moved) in [("minecraft:dirt_path", true), ("minecraft:farmland", false)] {
            let mut session = session();
            let target = IVec3::new(8, 2, 7);
            assert!(session.set(target + IVec3::Y, session.states[3]));
            let absolute = (session.origin + target).as_dvec3() + DVec3::new(0.5, 15.0 / 16.0, 0.5);
            session
                .dropped_items
                .spawn_saved(crate::saved_item::SavedItemState {
                    source: Arc::new(Tag::Compound(vec![])),
                    uuid: None,
                    position: absolute,
                    velocity: DVec3::ZERO,
                    on_ground: false,
                    no_gravity: true,
                    age: 0,
                    pickup_delay: i16::MAX,
                    health: 5,
                    fire_ticks: 0,
                    invulnerable: false,
                    invulnerable_time: 0,
                    owner: None,
                    thrower: None,
                    stack: ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![]))
                        .unwrap(),
                });
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(block.into(), 2, 64, Tag::Compound(vec![])),
            );
            session.set_player_inventory(inventory);
            session.submit_command(PlayerCommand::Place);
            session.update(TICK);
            assert_eq!(
                session.dropped_items.snapshot()[0].position.y,
                absolute.y + if moved { 1.0 / 16.0 } else { 0.0 },
                "{block}"
            );
            let absolute_block = session.origin + target;
            assert_eq!(
                session
                    .world
                    .block(absolute_block.x, absolute_block.y, absolute_block.z)
                    .unwrap()
                    .name,
                "minecraft:dirt"
            );
            assert_eq!(
                session
                    .inventory
                    .slot(PlayerSlot::Storage(0))
                    .unwrap()
                    .count(),
                1
            );
        }
    }

    #[test]
    fn soil_ticks_deduplicate_and_share_block_tick_work_limit() {
        let mut session = session();
        let soil = IVec3::new(8, 2, 6);
        let path = session.intern_block(state("minecraft:dirt_path", &[]));
        assert!(session.set(soil, path));
        assert!(session.set(soil + IVec3::Y, session.states[3]));
        assert!(session.set(soil + IVec3::Y, session.states[0]));
        assert!(session.set(soil + IVec3::Y, session.states[3]));
        assert_eq!(session.soil_ticks.len(), 1);

        session.tick = 1;
        let stale = SavedBlockTick {
            block: "minecraft:sand".into(),
            position: (session.origin + IVec3::new(9, 2, 7)).to_array(),
            delay: 0,
            priority: 0,
        };
        session.enqueue_saved_sand(&vec![stale; 65_536]);
        session.advance_block_ticks();
        assert_eq!(session.soil_ticks.len(), 1);
        assert!(
            matches!(cell(&session.world, session.origin, session.bounds, soil, session.states[0]), Cell::State(id) if id == path)
        );

        session.advance_block_ticks();
        assert!(session.soil_ticks.is_empty());
        assert!(session.soil_pending.is_empty());
        assert!(
            matches!(cell(&session.world, session.origin, session.bounds, soil, session.states[0]), Cell::State(id) if id == session.states[2])
        );
    }

    #[test]
    fn soil_tick_waits_for_its_chunk_and_runs_after_reload() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let remote = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let soil = IVec3::new(8, 2, 6);
        let absolute = session.origin + soil;
        let path = session.intern_block(state("minecraft:dirt_path", &[]));
        assert!(session.set(soil, path));
        assert!(session.set(soil + IVec3::Y, session.states[3]));
        session.queue_chunk_window(remote, 0);
        for _ in 0..5 {
            session.update(TICK);
        }
        assert_eq!(session.soil_deferred.len(), 1);
        assert!(session.draft_block_ticks(home).unwrap()[0].delay < 0);
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        assert!(session.soil_ticks.is_empty());
        assert!(session.soil_deferred.is_empty());
        assert!(session.soil_pending.is_empty());
        assert_eq!(
            session
                .world
                .block(absolute.x, absolute.y, absolute.z)
                .unwrap()
                .name,
            "minecraft:dirt"
        );
    }

    #[test]
    fn saved_soil_ticks_restore_duplicates_and_keep_their_delay() {
        let (mut world, origin, player) = fixture();
        let absolute = origin + IVec3::new(8, 2, 6);
        let chunk = ChunkPos {
            x: absolute.x.div_euclid(16),
            z: absolute.z.div_euclid(16),
        };
        let path = world.intern_state(state("minecraft:dirt_path", &[]));
        world
            .set_block_id(absolute.x, absolute.y, absolute.z, path)
            .unwrap();
        let tick = |delay| SavedBlockTick {
            block: "minecraft:dirt_path".into(),
            position: absolute.to_array(),
            delay,
            priority: 0,
        };
        let source = vec![tick(0), tick(4)];
        world.saved_block_ticks.insert(chunk, source.clone());
        let mut session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        assert_eq!(session.draft_block_ticks(chunk).unwrap(), source);
        assert_eq!(session.soil_ticks.len(), 2);
        assert_eq!(session.soil_pending.len(), 1);
        session.advance_block_ticks();
        assert_eq!(
            session
                .world
                .block(absolute.x, absolute.y, absolute.z)
                .unwrap()
                .name,
            "minecraft:dirt"
        );
        assert!(session.soil_pending.is_empty());
        assert_eq!(session.draft_block_ticks(chunk).unwrap(), vec![tick(4)]);
    }

    #[test]
    fn farmland_tick_is_preserved_when_its_cover_is_unknown() {
        let mut session = session();
        let relative = IVec3::new(8, 2, 6);
        let absolute = session.origin + relative;
        let farmland = session.intern_block(state("minecraft:farmland", &[("moisture", "0")]));
        let unknown = session.intern_block(state("custom:unknown", &[]));
        assert!(session.set(relative, farmland));
        assert!(session.set(relative + IVec3::Y, unknown));
        session.soil_ticks.clear();
        session.soil_pending.clear();
        let tick = SavedBlockTick {
            block: "minecraft:farmland".into(),
            position: absolute.to_array(),
            delay: 0,
            priority: 0,
        };
        session.enqueue_saved_soil(std::slice::from_ref(&tick));
        session.advance_block_ticks();
        assert_eq!(session.soil_deferred.len(), 1);
        assert!(session.soil_ticks.is_empty());
        for _ in 0..4 {
            session.advance_block_ticks();
            assert_eq!(session.soil_deferred.len(), 1);
            assert!(session.soil_ticks.is_empty());
        }
        assert_eq!(
            session
                .draft_block_ticks(session.player_chunk().unwrap())
                .unwrap(),
            vec![tick]
        );
        assert!(session.set(relative + IVec3::Y, session.states[0]));
        session.advance_block_ticks();
        assert!(session.soil_deferred.is_empty());
        assert!(session.soil_pending.is_empty());
        assert!(
            session
                .draft_block_ticks(session.player_chunk().unwrap())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn farmland_maintainer_tag_controls_placement_and_scheduled_ticks() {
        let mut session = session();
        let relative = IVec3::new(8, 2, 6);
        session.set_farmland_maintainers(Arc::new(BTreeSet::from(["minecraft:stone".into()])));
        assert!(session.set(relative + IVec3::Y, session.states[3]));
        assert_eq!(
            session.placement_block(relative, Block::Farmland(0)),
            Some(Block::Farmland(0))
        );
        assert_eq!(
            session.placement_block(relative, Block::DirtPath),
            Some(Block::Dirt)
        );
        let farmland = session.intern_block(state("minecraft:farmland", &[("moisture", "0")]));
        assert!(session.set(relative, farmland));
        let absolute = session.origin + relative;
        session.enqueue_saved_soil(&[SavedBlockTick {
            block: "minecraft:farmland".into(),
            position: absolute.to_array(),
            delay: 0,
            priority: 0,
        }]);
        session.advance_block_ticks();
        assert_eq!(
            session
                .world
                .block(absolute.x, absolute.y, absolute.z)
                .unwrap()
                .name,
            "minecraft:farmland"
        );
        session.set_farmland_maintainers(Arc::new(BTreeSet::new()));
        assert!(session.set(relative + IVec3::Y, session.states[0]));
        assert!(session.set(relative + IVec3::Y, session.states[3]));
        session.tick = 1;
        session.advance_block_ticks();
        assert_eq!(
            session
                .world
                .block(absolute.x, absolute.y, absolute.z)
                .unwrap()
                .name,
            "minecraft:dirt"
        );
    }

    #[test]
    fn vanilla_fence_gates_allow_dirt_path_underneath() {
        for wood in native_core::wood::Planks::ALL {
            let family = wood.id().strip_suffix("_planks").unwrap();
            let gate = format!("{family}_fence_gate");
            let mut session = session();
            let relative = IVec3::new(8, 2, 6);
            let gate_id = session.intern_block(state(&gate, &[]));
            assert!(session.set(relative + IVec3::Y, gate_id));
            assert_eq!(
                session.placement_block(relative, Block::DirtPath),
                Some(Block::DirtPath)
            );
            assert_eq!(
                session.placement_block(relative, Block::Farmland(0)),
                Some(Block::Dirt)
            );
            session.set_farmland_maintainers(Arc::new(BTreeSet::from([gate.clone()])));
            assert_eq!(
                session.placement_block(relative, Block::Farmland(0)),
                Some(Block::Farmland(0))
            );
            let path = session.intern_block(state("minecraft:dirt_path", &[]));
            assert!(session.set(relative, path));
            assert!(session.set(relative + IVec3::Y, session.states[0]));
            assert!(session.set(relative + IVec3::Y, gate_id));
            assert!(session.soil_ticks.is_empty());
            session.update(TICK * 3);
            assert!(
                matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(id) if id == path)
            );
            let farmland = session.intern_block(state("minecraft:farmland", &[("moisture", "0")]));
            assert!(session.set(relative, farmland));
            assert!(session.set(relative + IVec3::Y, session.states[0]));
            assert!(session.set(relative + IVec3::Y, gate_id));
            assert!(session.soil_ticks.is_empty());
            session.set_farmland_maintainers(Arc::new(BTreeSet::new()));
            assert!(session.set(relative + IVec3::Y, session.states[0]));
            assert!(session.set(relative + IVec3::Y, gate_id));
            assert_eq!(session.soil_ticks.len(), 1);
            session.update(TICK * 2);
            assert!(
                matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(id) if id == session.states[2])
            );
        }
        assert!(!is_vanilla_fence_gate("custom:oak_fence_gate"));
        assert!(!is_vanilla_fence_gate("minecraft:unknown_fence_gate"));
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn dirt_path_fence_gate_survival_matches_java() {
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
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SoilGateOracle.java");
        std::fs::write(&source, include_str!("../tests/java/SoilGateOracle.java")).unwrap();
        let output = temp.path().join("results.json");
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java soil oracle failed:\n{}\n{}",
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
            .arg("SoilGateOracle")
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let mut ids = BTreeSet::new();
        let session = session();
        let mut solid_values = BTreeSet::new();
        for row in &rows {
            let id = row["id"].as_str().unwrap();
            ids.insert(id.to_owned());
            solid_values.insert(row["solid"].as_bool().unwrap());
            assert_eq!(
                session.soil_cover(SoilKind::Path, id),
                Some(!row["path_survives"].as_bool().unwrap()),
                "{row}"
            );
            assert_eq!(
                session.soil_cover(SoilKind::Farmland, id),
                Some(row["solid"].as_bool().unwrap()),
                "{row}"
            );
        }
        let expected: BTreeSet<_> = native_core::wood::Planks::ALL
            .into_iter()
            .map(|wood| format!("{}_fence_gate", wood.id().strip_suffix("_planks").unwrap()))
            .collect();
        assert_eq!(ids, expected);
        println!(
            "Compared {} gate states; solid values: {:?}",
            rows.len(),
            solid_values
        );
    }

    #[test]
    fn farmland_tick_waits_until_maintenance_tag_is_available() {
        let mut session = session();
        session.farmland_maintainers = None;
        let relative = IVec3::new(8, 2, 6);
        let farmland = session.intern_block(state("minecraft:farmland", &[("moisture", "0")]));
        assert!(session.set(relative, farmland));
        assert!(session.set(relative + IVec3::Y, session.states[3]));
        session.tick = 1;
        session.advance_block_ticks();
        assert_eq!(session.soil_deferred.len(), 1);
        session.set_farmland_maintainers(Arc::new(BTreeSet::new()));
        session.advance_block_ticks();
        assert!(session.soil_deferred.is_empty());
        assert!(
            matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(id) if id == session.states[2])
        );
    }

    #[test]
    fn verified_slabs_and_stairs_cover_soil_and_unknown_states_are_withheld() {
        let cases = [
            state(
                "minecraft:oak_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            ),
            state(
                "minecraft:oak_stairs",
                &[
                    ("facing", "north"),
                    ("half", "bottom"),
                    ("shape", "outer_left"),
                    ("waterlogged", "false"),
                ],
            ),
        ];
        for cover in cases {
            for (kind, soil) in [
                (SoilKind::Path, Block::DirtPath),
                (SoilKind::Farmland, Block::Farmland(0)),
            ] {
                let mut session = session();
                session.set_vanilla_block_ids(Arc::new(BTreeSet::from([cover.name.clone()])));
                let relative = IVec3::new(8, 2, 6);
                let cover_id = session.intern_block(cover.clone());
                assert!(session.set(relative + IVec3::Y, cover_id));
                assert_eq!(session.placement_block(relative, soil), Some(Block::Dirt));
                let soil_id = session.placement_id(soil);
                assert!(session.set(relative, soil_id));
                assert!(session.set(relative + IVec3::Y, session.states[0]));
                assert!(session.set(relative + IVec3::Y, cover_id));
                assert_eq!(session.soil_ticks.len(), 1, "{kind:?} {cover:?}");
                session.update(TICK * 2);
                assert!(
                    matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(id) if id == session.states[2])
                );
            }
        }
        let mut session = session();
        session.set_vanilla_block_ids(Arc::new(BTreeSet::from(["minecraft:oak_slab".into()])));
        let invalid = state(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "invalid")],
        );
        let fake = state(
            "minecraft:unknown_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );
        assert_eq!(session.soil_cover_state(SoilKind::Path, &invalid), None);
        assert_eq!(session.soil_cover_state(SoilKind::Path, &fake), None);
        let valid = state(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );
        session.set_farmland_maintainers(Arc::new(BTreeSet::from([valid.name.clone()])));
        assert_eq!(
            session.soil_cover_state(SoilKind::Farmland, &valid),
            Some(false)
        );
    }

    #[test]
    fn loaded_soil_placement_checks_the_space_above() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        for item in ["minecraft:dirt_path", "minecraft:farmland"] {
            for covered in [false, true] {
                let mut session = session();
                let relative = IVec3::new(8, 2, 7);
                if covered {
                    assert!(session.set(relative + IVec3::Y, session.states[3]));
                }
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    PlayerSlot::Storage(0),
                    ItemStack::new(item.into(), 2, 64, Tag::Compound(vec![])),
                );
                session.set_player_inventory(inventory);
                session.submit_command(PlayerCommand::Place);
                session.update(TICK);
                let position = session.origin + relative;
                let snapshot = session.snapshot();
                let result = snapshot
                    .world
                    .loaded()
                    .unwrap()
                    .block(position.x, position.y, position.z)
                    .unwrap();
                assert_eq!(result.name, if covered { "minecraft:dirt" } else { item });
                assert_eq!(
                    snapshot
                        .inventory
                        .slot(PlayerSlot::Storage(0))
                        .unwrap()
                        .count(),
                    1
                );
            }
        }
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn loaded_rooted_dirt_interaction_respects_block_drops_rule() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        let (hoe, loot) = crate::tests::installed_hoe_and_loot();
        for enabled in [false, true] {
            for mode in [GameMode::Survival, GameMode::Creative] {
                let mut session = session();
                session.set_game_mode(mode);
                session.set_block_drops(enabled);
                session.set_tool_transformers(ToolKind::Hoe, Arc::clone(&hoe));
                session.set_block_loot(Arc::clone(&loot));
                let relative = IVec3::new(8, 2, 6);
                let id = session.intern_block(state("minecraft:rooted_dirt", &[]));
                assert!(session.set(relative, id));
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    PlayerSlot::Storage(0),
                    ItemStack::new("minecraft:wooden_hoe".into(), 1, 1, Tag::Compound(vec![])),
                );
                session.set_player_inventory(inventory);
                let before = session.snapshot();
                let absolute = session.origin + relative;
                assert!(session.use_tool(relative, IVec3::Z));
                assert_eq!(
                    session
                        .world
                        .block(absolute.x, absolute.y, absolute.z)
                        .unwrap()
                        .name,
                    "minecraft:dirt"
                );
                assert_eq!(
                    before
                        .world
                        .loaded()
                        .unwrap()
                        .block(absolute.x, absolute.y, absolute.z)
                        .unwrap()
                        .name,
                    "minecraft:rooted_dirt"
                );
                let drops = session.dropped_items.snapshot();
                assert_eq!(drops.len(), usize::from(enabled));
                if enabled {
                    assert_eq!(drops[0].stack.id(), "minecraft:hanging_roots");
                    assert!((drops[0].position.z - (f64::from(absolute.z) + 1.125)).abs() < 1.0e-9);
                }
            }
        }
    }

    #[test]
    fn loaded_world_break_respects_the_rule_and_keeps_item_positions_absolute() {
        use native_resources::{loot::BlockLootTables, packs::ResourceProvider};
        struct LootSource;
        impl ResourceProvider for LootSource {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(path.ends_with("/dirt.json").then(|| br#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:dirt"}]}]}"#.to_vec()))
            }
        }
        let loot = Arc::new(BlockLootTables::load(&mut LootSource).unwrap());
        for enabled in [false, true] {
            let mut session = session();
            session.set_block_loot(Arc::clone(&loot));
            session.set_block_drops(enabled);
            let relative = IVec3::new(8, 2, 6);
            assert!(session.set(relative, session.states[2]));
            let absolute = session.origin + relative;
            session.finish_breaking(absolute);
            session.update(TICK);
            let snapshot = session.snapshot();
            assert_eq!(snapshot.dropped_items.len(), usize::from(enabled));
            if enabled {
                let item = &snapshot.dropped_items[0];
                assert_eq!(item.stack.id(), "minecraft:dirt");
                assert!((item.position.x - f64::from(absolute.x)).abs() < 1.0);
                assert!((item.position.z - f64::from(absolute.z)).abs() < 1.0);
            }
        }
    }

    #[test]
    fn loaded_mining_applies_data_pack_tool_durability() {
        use native_resources::packs::{ResourceId, ResourceProvider};
        struct Data(f32);
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, path: &str) -> anyhow::Result<Vec<ResourceId>> {
                Ok(if path == "enchantment" {
                    vec![ResourceId::parse("minecraft:unbreaking")?]
                } else {
                    vec![]
                })
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(path.ends_with("/enchantment/unbreaking.json").then(|| format!(r#"{{"slots":["any"],"effects":{{"minecraft:item_damage":[{{"effect":{{"type":"minecraft:remove_binomial","chance":{}}}}}]}}}}"#, self.0).into_bytes()))
            }
        }
        for chance in [0.0, 1.0] {
            let mut session = session();
            let tool = crate::inventory::ItemStack::new(
                "minecraft:iron_shovel".into(),
                1,
                1,
                native_persistence::nbt::Tag::Compound(vec![(
                    "minecraft:enchantments".into(),
                    native_persistence::nbt::Tag::Compound(vec![(
                        "minecraft:unbreaking".into(),
                        native_persistence::nbt::Tag::Int(3),
                    )]),
                )]),
            )
            .unwrap();
            Arc::make_mut(&mut session.inventory)
                .set_slot(crate::inventory::PlayerSlot::Storage(0), Some(tool));
            session
                .set_saved_breathing(
                    Default::default(),
                    crate::player_data::SavedAttribute::with_base(0.0),
                    Arc::new(OxygenEnchantments::load(&mut Data(chance)).unwrap()),
                )
                .unwrap();
            let relative = IVec3::new(8, 2, 6);
            let dirt = session.intern_block(state("minecraft:dirt", &[]));
            assert!(session.set(relative + IVec3::NEG_Y, session.states[3]));
            assert!(session.set(relative, dirt));
            session.update(TICK * 3);
            session.submit_command(PlayerCommand::Break);
            session.update(TICK * 10);
            assert!(
                !matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(id) if id == dirt)
            );
            let tool = session
                .inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap();
            assert_eq!(
                tool.components().get("minecraft:damage"),
                if chance == 0.0 {
                    Some(&native_persistence::nbt::Tag::Int(1))
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn loaded_mining_applies_haste_and_fatigue() {
        fn ticks_to_break(haste: bool, fatigue: bool) -> usize {
            let mut session = session();
            let relative = IVec3::new(8, 2, 6);
            let dirt = session.intern_block(state("minecraft:dirt", &[]));
            assert!(session.set(relative + IVec3::NEG_Y, session.states[3]));
            assert!(session.set(relative, dirt));
            session.update(TICK * 3);
            let effect = || crate::survival::EffectDuration {
                amplifier: 0,
                duration: 1000,
                hidden: None,
            };
            session.fall.configure(
                crate::player_data::SavedAttribute::with_base(3.0),
                crate::player_data::SavedAttribute::with_base(1.0),
                crate::survival::FallEffects {
                    haste: haste.then(effect),
                    mining_fatigue: fatigue.then(effect),
                    ..Default::default()
                },
                Arc::new(OxygenEnchantments::default()),
            );
            session.submit_command(PlayerCommand::Break);
            for tick in 1..=100 {
                session.update(TICK);
                if !matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(id) if id == dirt)
                {
                    return tick;
                }
            }
            panic!("block was not broken")
        }
        let normal = ticks_to_break(false, false);
        let fast = ticks_to_break(true, false);
        let slow = ticks_to_break(false, true);
        assert!(fast < normal && normal < slow, "{fast} < {normal} < {slow}");
    }

    #[test]
    fn loaded_sand_uses_mining_progress_and_spawns_its_loot() {
        use native_resources::{loot::BlockLootTables, packs::ResourceProvider};
        struct LootSource;
        impl ResourceProvider for LootSource {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(path.ends_with("/sand.json").then(|| br#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:sand"}]}]}"#.to_vec()))
            }
        }
        let mut session = session();
        session.set_block_loot(Arc::new(BlockLootTables::load(&mut LootSource).unwrap()));
        let relative = IVec3::new(8, 2, 6);
        let sand = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(relative + IVec3::NEG_Y, session.states[3]));
        assert!(session.set(relative, sand));
        session.update(TICK * 3);
        session.submit_command(PlayerCommand::Break);
        session.update(TICK);
        assert!(
            matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(id) if id == sand)
        );
        for _ in 0..80 {
            session.update(TICK);
        }
        assert!(
            !matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(id) if id == sand)
        );
        assert!(
            session
                .snapshot()
                .dropped_items
                .iter()
                .any(|item| item.stack.id() == "minecraft:sand")
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn loaded_enchanted_tools_apply_real_loot_and_durability() {
        use native_persistence::nbt::Tag;
        use native_resources::{loot::BlockLootTables, packs::ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let loot = Arc::new(BlockLootTables::load(&mut resources).unwrap());
        let enchantments = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
        for (block, item, enchantment, level, expected) in [
            ("stone", "iron_pickaxe", "silk_touch", 1, "stone"),
            ("gravel", "iron_shovel", "fortune", 3, "flint"),
        ] {
            let mut session = session();
            session.set_block_loot(Arc::clone(&loot));
            let components = Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![(
                    format!("minecraft:{enchantment}").as_str().into(),
                    Tag::Int(level),
                )]),
            )]);
            let tool =
                crate::inventory::ItemStack::new(format!("minecraft:{item}"), 1, 1, components)
                    .unwrap();
            Arc::make_mut(&mut session.inventory)
                .set_slot(crate::inventory::PlayerSlot::Storage(0), Some(tool));
            session
                .set_saved_breathing(
                    Default::default(),
                    crate::player_data::SavedAttribute::with_base(0.0),
                    Arc::clone(&enchantments),
                )
                .unwrap();
            let relative = IVec3::new(8, 2, 6);
            let id = session.intern_block(state(&format!("minecraft:{block}"), &[]));
            assert!(session.set(relative + IVec3::NEG_Y, session.states[3]));
            session.set(relative, id);
            assert!(
                matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(current) if current == id)
            );
            session.update(TICK * 3);
            session.submit_command(PlayerCommand::Break);
            for _ in 0..40 {
                session.update(TICK);
                if session
                    .snapshot()
                    .dropped_items
                    .iter()
                    .any(|drop| drop.stack.id() == format!("minecraft:{expected}"))
                {
                    break;
                }
            }
            assert!(
                session
                    .snapshot()
                    .dropped_items
                    .iter()
                    .any(|drop| drop.stack.id() == format!("minecraft:{expected}")),
                "{block}"
            );
            let tool = session
                .inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap();
            assert_eq!(
                tool.components().get("minecraft:damage"),
                Some(&Tag::Int(1)),
                "{block}"
            );
        }
    }

    #[test]
    fn loaded_gravel_mining_uses_data_pack_random_loot() {
        use native_resources::{loot::BlockLootTables, packs::ResourceProvider};
        struct LootSource;
        impl ResourceProvider for LootSource {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(path.ends_with("/gravel.json").then(|| br#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:alternatives","children":[{"type":"minecraft:item","condition":{"type":"minecraft:table_bonus","enchantment":"minecraft:fortune","chances":[1.0]},"name":"minecraft:flint"},{"type":"minecraft:item","name":"minecraft:gravel"}]}]}]}"#.to_vec()))
            }
        }
        let mut session = session();
        session.set_block_loot(Arc::new(BlockLootTables::load(&mut LootSource).unwrap()));
        let relative = IVec3::new(8, 2, 6);
        let gravel = session.intern_block(state("minecraft:gravel", &[]));
        assert!(session.set(relative + IVec3::NEG_Y, session.states[3]));
        assert!(session.set(relative, gravel));
        session.update(TICK * 3);
        session.submit_command(PlayerCommand::Break);
        session.update(TICK);
        assert!(
            matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(id) if id == gravel)
        );
        for _ in 0..80 {
            session.update(TICK);
            if matches!(cell(&session.world, session.origin, session.bounds, relative, session.states[0]), Cell::State(id) if id == session.states[0])
            {
                break;
            }
        }
        assert!(
            session
                .snapshot()
                .dropped_items
                .iter()
                .any(|item| item.stack.id() == "minecraft:flint")
        );
    }

    #[test]
    fn loaded_world_break_uses_last_tool_durability_and_preserves_prior_snapshot() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        use native_resources::{loot::BlockLootTables, packs::ResourceProvider};

        struct LootSource;
        impl ResourceProvider for LootSource {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(path.ends_with("/stone.json").then(|| br#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:cobblestone"}]}]}"#.to_vec()))
            }
        }
        let loot = Arc::new(BlockLootTables::load(&mut LootSource).unwrap());
        for drops_enabled in [false, true] {
            let mut session = session();
            session.set_block_loot(Arc::clone(&loot));
            session.set_block_drops(drops_enabled);
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(
                    "minecraft:stone_pickaxe".into(),
                    1,
                    1,
                    Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(130))]),
                ),
            );
            session.set_player_inventory(inventory);
            session.update(TICK * 3);
            let before = session.snapshot();
            let target = session.origin + IVec3::new(8, 2, 6);
            session.submit_command(PlayerCommand::Break);
            for _ in 0..30 {
                session.update(TICK);
                if session
                    .snapshot()
                    .world
                    .loaded()
                    .unwrap()
                    .block(target.x, target.y, target.z)
                    .unwrap()
                    .name
                    == "minecraft:air"
                {
                    break;
                }
            }
            let after = session.snapshot();
            assert_eq!(
                after
                    .world
                    .loaded()
                    .unwrap()
                    .block(target.x, target.y, target.z)
                    .unwrap()
                    .name,
                "minecraft:air"
            );
            assert!(after.inventory.slot(PlayerSlot::Storage(0)).is_none());
            assert_eq!(
                before
                    .inventory
                    .slot(PlayerSlot::Storage(0))
                    .unwrap()
                    .components()
                    .get("minecraft:damage"),
                Some(&Tag::Int(130))
            );
            assert_eq!(after.dropped_items.len(), usize::from(drops_enabled));
            if drops_enabled {
                assert_eq!(after.dropped_items[0].stack.id(), "minecraft:cobblestone");
            }
        }
    }

    #[test]
    fn player_drop_uses_absolute_world_coordinates_and_respects_spectator_mode() {
        let mut session = session();
        let before = session.snapshot();
        session.submit_command(PlayerCommand::DropSelected { all: false });
        session.update(TICK);
        let after = session.snapshot();
        assert_eq!(after.dropped_items.len(), 1);
        let absolute_player = before.origin.as_dvec3() + before.player.position.as_dvec3();
        let item = &after.dropped_items[0];
        assert!((item.position.x - absolute_player.x).abs() < 1.0);
        assert!((item.position.z - absolute_player.z).abs() < 1.0);
        assert_eq!(
            after
                .inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            63
        );
        assert_eq!(
            before
                .inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            64
        );
        session.set_game_mode(GameMode::Spectator);
        session.submit_command(PlayerCommand::DropSelected { all: true });
        session.update(TICK);
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            63
        );
    }

    #[test]
    fn loaded_world_swaps_hands_and_hovered_slots_on_ticks() {
        use crate::inventory::PlayerSlot;
        let mut session = session();
        let before = session.snapshot();
        session.submit_command(PlayerCommand::SwapHands);
        session.update(TICK);
        let swapped = session.snapshot();
        assert_eq!(
            swapped.inventory.slot(PlayerSlot::Offhand),
            before.inventory.slot(PlayerSlot::Storage(0))
        );
        assert!(swapped.inventory.slot(PlayerSlot::Storage(0)).is_none());
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::InventorySwap {
            slot: PlayerSlot::Storage(1),
            target: PlayerSlot::Offhand,
        });
        session.update(TICK);
        let inventory = session.snapshot();
        assert_eq!(
            inventory.inventory.slot(PlayerSlot::Storage(1)),
            before.inventory.slot(PlayerSlot::Storage(0))
        );
        assert_eq!(
            inventory.inventory.slot(PlayerSlot::Offhand),
            before.inventory.slot(PlayerSlot::Storage(1))
        );
        assert!(before.inventory.slot(PlayerSlot::Offhand).is_none());
    }

    fn liquid_session(name: &str) -> LoadedSession {
        let (mut world, origin, mut player) = fixture();
        let liquid = world.intern_state(state(name, &[("level", "0")]));
        let pos = ChunkPos {
            x: origin.x.div_euclid(16),
            z: origin.z.div_euclid(16),
        };
        let chunk = Arc::make_mut(world.chunks.get_mut(&pos).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        let blocks = section.blocks.as_mut().unwrap();
        for x in 3..13 {
            for z in 3..13 {
                for y in 1..6 {
                    blocks[x + z * 16 + y * 256] = liquid;
                }
            }
        }
        player.position.y = 2.0;
        LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap()
    }

    #[test]
    fn creative_flight_ignores_fluid_movement_and_uses_flight_ascent() {
        for name in ["minecraft:water", "minecraft:lava"] {
            let mut session = liquid_session(name);
            session.set_game_mode(GameMode::Creative);
            session.flight.flying = true;
            session.motion.position = DVec3::new(4.5, 2.0, 4.5);
            session.motion.velocity = DVec3::new(0.3, 0.0, 0.0);
            session.motion.on_ground = false;
            session.player.position = session.motion.position.as_vec3();
            session.submit_command(PlayerCommand::Move {
                forward: 0.0,
                strafe: 0.0,
                jump: true,
            });
            session.update(TICK);
            assert!(
                (session.motion.velocity.x - 0.3 * f64::from(0.91_f32)).abs() < 1e-12,
                "{name}"
            );
            assert!(
                (session.motion.velocity.y - f64::from(session.flight_speed * 3.0_f32) * 0.6).abs()
                    < 1e-12,
                "{name}"
            );
        }
    }

    #[test]
    fn saved_item_uses_loaded_world_fluid_contacts() {
        for name in ["minecraft:water", "minecraft:lava"] {
            let mut session = liquid_session(name);
            let position = session.origin.as_dvec3() + DVec3::new(4.5, 2.0, 4.5);
            session
                .dropped_items
                .spawn_saved(crate::saved_item::SavedItemState {
                    source: Arc::new(native_persistence::nbt::Tag::Compound(vec![])),
                    uuid: None,
                    position,
                    velocity: DVec3::ZERO,
                    on_ground: false,
                    no_gravity: false,
                    age: 0,
                    pickup_delay: 10,
                    health: 5,
                    fire_ticks: 0,
                    invulnerable: false,
                    invulnerable_time: 0,
                    owner: None,
                    thrower: None,
                    stack: crate::inventory::ItemStack::new(
                        "minecraft:stone".into(),
                        1,
                        64,
                        native_persistence::nbt::Tag::Compound(vec![]),
                    )
                    .unwrap(),
                });
            session.update(TICK);
            let item = &session.snapshot().dropped_items[0];
            assert!(
                (item.position.y - position.y - f64::from(0.0005_f32)).abs() < 1.0e-10,
                "{name}"
            );
        }
    }

    #[test]
    fn loaded_item_bounces_on_slime_but_not_stone() {
        for (name, suppressed, expected_bounce, x) in [
            ("minecraft:slime_block", false, true, 4.5),
            ("minecraft:slime_block", false, true, 5.02),
            ("minecraft:slime_block", true, false, 5.02),
            ("minecraft:stone", false, false, 5.02),
        ] {
            let mut session = session();
            if suppressed {
                session.set_bounce_suppression_tag(Arc::new(BTreeSet::from([name.into()])));
            }
            let block = session.intern_block(state(name, &[]));
            assert!(session.set(IVec3::new(4, 1, 4), block));
            let position = session.origin.as_dvec3() + DVec3::new(x, 2.2, 4.5);
            session
                .dropped_items
                .spawn_saved(crate::saved_item::SavedItemState {
                    source: Arc::new(native_persistence::nbt::Tag::Compound(vec![])),
                    uuid: None,
                    position,
                    velocity: DVec3::new(0.0, -0.5, 0.0),
                    on_ground: false,
                    no_gravity: false,
                    age: 0,
                    pickup_delay: 10,
                    health: 5,
                    fire_ticks: 0,
                    invulnerable: false,
                    invulnerable_time: 0,
                    owner: None,
                    thrower: None,
                    stack: ItemStack::new(
                        "minecraft:stone".into(),
                        1,
                        64,
                        native_persistence::nbt::Tag::Compound(vec![]),
                    )
                    .unwrap(),
                });
            session.update(TICK);
            let landed = session.snapshot().dropped_items[0].position.y;
            for _ in 0..4 {
                session.update(TICK);
            }
            let next = session.snapshot().dropped_items[0].position.y;
            assert_eq!(
                next > landed + 0.001,
                expected_bounce,
                "{name} x={x}: {landed} -> {next}"
            );
        }
    }

    #[test]
    fn loaded_orb_uses_support_friction_at_block_edges() {
        for (name, friction) in [
            ("minecraft:slime_block", 0.8_f32),
            ("minecraft:stone", 0.6_f32),
        ] {
            let mut session = session();
            session.set_game_mode(GameMode::Spectator);
            let block = session.intern_block(state(name, &[]));
            assert!(session.set(IVec3::new(4, 1, 4), block));
            let position = session.origin.as_dvec3() + DVec3::new(5.05, 2.2, 4.5);
            session
                .experience_orbs
                .spawn_saved(crate::saved_orb::SavedOrb {
                    source: Arc::new(native_persistence::nbt::Tag::Compound(vec![])),
                    uuid: None,
                    position,
                    velocity: DVec3::new(0.02, -0.5, 0.0),
                    on_ground: false,
                    no_gravity: false,
                    age: 0,
                    value: 7,
                    count: 1,
                    health: 5,
                    fire_ticks: 0,
                    invulnerable: false,
                    invulnerable_time: 0,
                });
            session.update(TICK);
            let landed = session.snapshot().experience_orbs[0].position;
            session.update(TICK);
            let next = session.snapshot().experience_orbs[0].position;
            let expected_dx = 0.02 * f64::from(0.98_f32 * friction);
            assert!(
                (next.x - landed.x - expected_dx).abs() < 1.0e-8,
                "{name}: {landed:?} -> {next:?}"
            );
            assert!(next.y > landed.y, "{name}: {landed:?} -> {next:?}");
        }
    }

    #[test]
    fn saved_experience_orb_uses_loaded_water_and_lava() {
        for (name, expected_y) in [
            ("minecraft:water", f64::from(0.0005_f32)),
            ("minecraft:lava", f64::from(0.2_f32)),
        ] {
            for no_gravity in [false, true] {
                let mut session = liquid_session(name);
                session.game_mode = GameMode::Spectator;
                let position = session.origin.as_dvec3() + DVec3::new(4.5, 2.0, 4.5);
                session
                    .experience_orbs
                    .spawn_saved(crate::saved_orb::SavedOrb {
                        source: Arc::new(native_persistence::nbt::Tag::Compound(vec![])),
                        uuid: None,
                        position,
                        velocity: DVec3::ZERO,
                        on_ground: false,
                        no_gravity,
                        age: 0,
                        value: 7,
                        count: 1,
                        health: 5,
                        fire_ticks: 0,
                        invulnerable: false,
                        invulnerable_time: 0,
                    });
                session.update(TICK);
                let orb = session.snapshot().experience_orbs[0];
                assert!(
                    (orb.position.y - position.y - expected_y).abs() < 1.0e-10,
                    "{name}, no gravity {no_gravity}"
                );
            }
        }
    }

    #[test]
    fn orb_with_eyes_above_water_applies_gravity() {
        let mut session = liquid_session("minecraft:water");
        session.game_mode = GameMode::Spectator;
        let position = session.origin.as_dvec3() + DVec3::new(4.5, 5.5, 4.5);
        session
            .experience_orbs
            .spawn_saved(crate::saved_orb::SavedOrb {
                source: Arc::new(native_persistence::nbt::Tag::Compound(vec![])),
                uuid: None,
                position,
                velocity: DVec3::ZERO,
                on_ground: false,
                no_gravity: false,
                age: 0,
                value: 7,
                count: 1,
                health: 5,
                fire_ticks: 0,
                invulnerable: false,
                invulnerable_time: 0,
            });
        session.update(TICK);
        let orb = session.snapshot().experience_orbs[0];
        assert!((orb.position.y - (position.y - 0.03)).abs() < 1.0e-10);
    }

    #[test]
    fn orb_draft_rejects_lava_pending_complete_validation() {
        use native_persistence::nbt::{Tag, TagType};
        let mut session = liquid_session("minecraft:lava");
        session.game_mode = GameMode::Spectator;
        let position = session.origin.as_dvec3() + DVec3::new(4.5, 2.0, 4.5);
        let source = Arc::new(Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:experience_orb".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: TagType::Double,
                    values: position.to_array().into_iter().map(Tag::Double).collect(),
                },
            ),
            ("Value".into(), Tag::Short(7)),
            ("Count".into(), Tag::Int(1)),
        ]));
        session
            .experience_orbs
            .spawn_saved(crate::saved_orb::SavedOrb {
                source,
                uuid: Some([1, 2, 3, 4]),
                position,
                velocity: DVec3::ZERO,
                on_ground: false,
                no_gravity: false,
                age: 0,
                value: 7,
                count: 1,
                health: 5,
                fire_ticks: 0,
                invulnerable: false,
                invulnerable_time: 0,
            });
        assert!(
            session
                .draft_saved_orb_entity([1, 2, 3, 4])
                .unwrap()
                .is_some()
        );
        session.update(TICK);
        assert!(session.draft_saved_orb_entity([1, 2, 3, 4]).is_err());
    }

    #[test]
    fn ordinary_orb_in_lava_is_destroyed_after_two_contacts() {
        let mut session = liquid_session("minecraft:lava");
        session.game_mode = GameMode::Spectator;
        let position = session.origin.as_dvec3() + DVec3::new(4.5, 2.0, 4.5);
        session
            .experience_orbs
            .spawn_saved(crate::saved_orb::SavedOrb {
                source: Arc::new(native_persistence::nbt::Tag::Compound(vec![])),
                uuid: None,
                position,
                velocity: DVec3::ZERO,
                on_ground: false,
                no_gravity: false,
                age: 0,
                value: 7,
                count: 1,
                health: 5,
                fire_ticks: 0,
                invulnerable: false,
                invulnerable_time: 0,
            });
        session.update(TICK);
        assert_eq!(session.snapshot().experience_orbs.len(), 1);
        session.update(TICK);
        assert!(session.snapshot().experience_orbs.is_empty());
    }

    #[test]
    fn orb_draft_rejects_lava_crossed_between_tick_endpoints() {
        use native_persistence::nbt::{Tag, TagType};
        let mut session = session();
        session.game_mode = GameMode::Spectator;
        let lava = session.intern_liquid(FluidState {
            kind: FluidKind::Lava,
            amount: 8,
            source: true,
            falling: false,
        });
        assert!(session.set(IVec3::new(4, 2, 4), lava));
        let position = session.origin.as_dvec3() + DVec3::new(1.5, 2.0, 4.5);
        let velocity = DVec3::new(6.0, 0.0, 0.0);
        let vector = |v: DVec3| Tag::List {
            element_type: TagType::Double,
            values: v.to_array().into_iter().map(Tag::Double).collect(),
        };
        let source = Arc::new(Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:experience_orb".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            ("Pos".into(), vector(position)),
            ("Motion".into(), vector(velocity)),
            ("Value".into(), Tag::Short(7)),
            ("Count".into(), Tag::Int(1)),
        ]));
        session
            .experience_orbs
            .spawn_saved(crate::saved_orb::SavedOrb {
                source,
                uuid: Some([1, 2, 3, 4]),
                position,
                velocity,
                on_ground: false,
                no_gravity: true,
                age: 0,
                value: 7,
                count: 1,
                health: 5,
                fire_ticks: 0,
                invulnerable: false,
                invulnerable_time: 0,
            });
        assert!(
            session
                .draft_saved_orb_entity([1, 2, 3, 4])
                .unwrap()
                .is_some()
        );
        session.update(TICK);
        let orb = session.snapshot().experience_orbs[0];
        assert_eq!(
            orb.position.x,
            position.x + 6.0,
            "orb must end beyond the lava cell"
        );
        assert!(session.draft_saved_orb_entity([1, 2, 3, 4]).is_err());
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn lava_damages_loaded_items_with_vanilla_resistance() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let items = Arc::new(resources.vanilla_item_registry().unwrap());
        let tags = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
        for (id, removed) in [
            ("minecraft:stone", true),
            ("minecraft:netherite_ingot", false),
            ("minecraft:nether_star", false),
        ] {
            let mut session = liquid_session("minecraft:lava");
            session.set_item_damage_rules(Arc::clone(&items), Arc::clone(&tags));
            let position = session.origin.as_dvec3() + DVec3::new(4.5, 2.0, 4.5);
            session
                .dropped_items
                .spawn_saved(crate::saved_item::SavedItemState {
                    source: Arc::new(native_persistence::nbt::Tag::Compound(vec![])),
                    uuid: None,
                    position,
                    velocity: DVec3::ZERO,
                    on_ground: false,
                    no_gravity: true,
                    age: 0,
                    pickup_delay: 10,
                    health: 5,
                    fire_ticks: 0,
                    invulnerable: false,
                    invulnerable_time: 0,
                    owner: None,
                    thrower: None,
                    stack: ItemStack::new(
                        id.into(),
                        1,
                        64,
                        native_persistence::nbt::Tag::Compound(vec![]),
                    )
                    .unwrap(),
                });
            session.update(TICK);
            assert_eq!(session.snapshot().dropped_items.len(), 1, "{id}");
            session.update(TICK);
            assert_eq!(session.snapshot().dropped_items.is_empty(), removed, "{id}");
        }
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn burning_item_takes_damage_and_water_extinguishes_it() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let items = Arc::new(resources.vanilla_item_registry().unwrap());
        let tags = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
        for wet in [false, true] {
            let mut session = if wet {
                liquid_session("minecraft:water")
            } else {
                session()
            };
            session.set_item_damage_rules(Arc::clone(&items), Arc::clone(&tags));
            let position = session.origin.as_dvec3() + DVec3::new(4.5, 2.0, 4.5);
            session
                .dropped_items
                .spawn_saved(crate::saved_item::SavedItemState {
                    source: Arc::new(native_persistence::nbt::Tag::Compound(vec![])),
                    uuid: None,
                    position,
                    velocity: DVec3::ZERO,
                    on_ground: false,
                    no_gravity: true,
                    age: 0,
                    pickup_delay: i16::MAX,
                    health: 1,
                    fire_ticks: if wet { 21 } else { 20 },
                    invulnerable: false,
                    invulnerable_time: 0,
                    owner: None,
                    thrower: None,
                    stack: ItemStack::new(
                        "minecraft:stone".into(),
                        1,
                        64,
                        native_persistence::nbt::Tag::Compound(vec![]),
                    )
                    .unwrap(),
                });
            for _ in 0..25 {
                session.update(TICK);
            }
            assert_eq!(
                session.snapshot().dropped_items.len(),
                usize::from(wet),
                "wet={wet}"
            );
        }
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn entering_water_or_lava_applies_the_fluid_effect_on_that_tick() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let items = Arc::new(resources.vanilla_item_registry().unwrap());
        let tags = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
        for liquid in ["minecraft:water", "minecraft:lava"] {
            let mut session = liquid_session(liquid);
            session.set_item_damage_rules(Arc::clone(&items), Arc::clone(&tags));
            let position = session.origin.as_dvec3() + DVec3::new(2.7, 2.0, 4.5);
            session
                .dropped_items
                .spawn_saved(crate::saved_item::SavedItemState {
                    source: Arc::new(native_persistence::nbt::Tag::Compound(vec![])),
                    uuid: None,
                    position,
                    velocity: DVec3::new(0.4, 0.0, 0.0),
                    on_ground: false,
                    no_gravity: true,
                    age: 0,
                    pickup_delay: i16::MAX,
                    health: if liquid == "minecraft:lava" { 4 } else { 1 },
                    fire_ticks: if liquid == "minecraft:water" { 21 } else { 0 },
                    invulnerable: false,
                    invulnerable_time: 0,
                    owner: None,
                    thrower: None,
                    stack: ItemStack::new(
                        "minecraft:stone".into(),
                        1,
                        64,
                        native_persistence::nbt::Tag::Compound(vec![]),
                    )
                    .unwrap(),
                });
            session.update(TICK);
            assert_eq!(
                session.snapshot().dropped_items.is_empty(),
                liquid == "minecraft:lava",
                "{liquid}"
            );
            if liquid == "minecraft:water" {
                for _ in 0..22 {
                    session.update(TICK);
                }
                assert_eq!(session.snapshot().dropped_items.len(), 1);
            }
        }
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn fast_item_crossing_lava_then_water_is_damaged_and_extinguished() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let items = Arc::new(resources.vanilla_item_registry().unwrap());
        let tags = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
        let (mut world, origin, player) = fixture();
        let lava = world.intern_state(state("minecraft:lava", &[("level", "0")]));
        let water = world.intern_state(state("minecraft:water", &[("level", "0")]));
        world
            .set_block_id(origin.x + 4, 2, origin.z + 4, lava)
            .unwrap();
        world
            .set_block_id(origin.x + 6, 2, origin.z + 4, water)
            .unwrap();
        let mut session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session.set_item_damage_rules(items, tags);
        let position = origin.as_dvec3() + DVec3::new(2.5, 2.0, 4.5);
        session
            .dropped_items
            .spawn_saved(crate::saved_item::SavedItemState {
                source: Arc::new(native_persistence::nbt::Tag::Compound(vec![])),
                uuid: None,
                position,
                velocity: DVec3::new(6.0, 0.0, 0.0),
                on_ground: false,
                no_gravity: true,
                age: 0,
                pickup_delay: i16::MAX,
                health: 5,
                fire_ticks: 0,
                invulnerable: false,
                invulnerable_time: 0,
                owner: None,
                thrower: None,
                stack: ItemStack::new(
                    "minecraft:stone".into(),
                    1,
                    64,
                    native_persistence::nbt::Tag::Compound(vec![]),
                )
                .unwrap(),
            });
        session.update(TICK);
        assert_eq!(session.snapshot().dropped_items.len(), 1);
        assert_eq!(session.dropped_items.debug_damage_states(), vec![(1, 0)]);
        assert!(session.snapshot().dropped_items[0].position.x > f64::from(origin.x) + 7.0);
        // If the later water did not extinguish the item, the first on-fire
        // damage tick after lava contact would destroy its remaining health.
        session.update(TICK);
        assert_eq!(session.snapshot().dropped_items.len(), 1);
    }

    fn fluid_flow_session(kind: FluidKind, fast: bool) -> LoadedSession {
        let (mut world, origin, player) = fixture();
        if fast {
            world.dimension = "minecraft:the_nether".into();
        }
        let name = match kind {
            FluidKind::Water => "minecraft:water",
            FluidKind::Lava => "minecraft:lava",
        };
        let fluid = world.intern_state(state(name, &[("level", "0")]));
        let stone = world.intern_state(state("minecraft:stone", &[]));
        let pos = ChunkPos {
            x: origin.x.div_euclid(16),
            z: origin.z.div_euclid(16),
        };
        let chunk = Arc::make_mut(world.chunks.get_mut(&pos).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        let blocks = section.blocks.as_mut().unwrap();
        blocks[8 + 8 * 16 + 2 * 256] = fluid;
        blocks[8 + 8 * 16 + 256] = stone;
        blocks[9 + 8 * 16 + 2 * 256] = stone;
        LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap()
    }

    #[test]
    fn water_and_waterlogged_blocks_solidify_neighboring_lava() {
        for (level, waterlogged, expected) in [
            (0, false, "minecraft:obsidian"),
            (3, false, "minecraft:cobblestone"),
            (8, true, "minecraft:cobblestone"),
            (0, true, "minecraft:obsidian"),
        ] {
            let mut session = fluid_flow_session(FluidKind::Lava, false);
            let lava = session.intern_liquid(FluidState {
                kind: FluidKind::Lava,
                amount: if level == 0 || level >= 8 {
                    8
                } else {
                    8 - level
                },
                source: level == 0,
                falling: level >= 8,
            });
            session.set(IVec3::new(8, 2, 8), lava);
            let touching = if waterlogged {
                session.intern_block(state(
                    "minecraft:oak_slab",
                    &[("type", "bottom"), ("waterlogged", "true")],
                ))
            } else {
                session.intern_liquid(FluidState {
                    kind: FluidKind::Water,
                    amount: 8,
                    source: true,
                    falling: false,
                })
            };
            session.set(IVec3::new(9, 2, 8), touching);
            let before_tick = session
                .world
                .block(session.origin.x + 8, 2, session.origin.z + 8)
                .unwrap();
            assert_eq!(before_tick.name, expected);
            assert_eq!(
                session
                    .world
                    .block(session.origin.x + 9, 2, session.origin.z + 8)
                    .unwrap()
                    .name,
                if waterlogged {
                    "minecraft:oak_slab"
                } else {
                    "minecraft:water"
                }
            );
            session.update(TICK);
            assert_eq!(
                session
                    .snapshot()
                    .world
                    .loaded()
                    .unwrap()
                    .block(session.origin.x + 8, 2, session.origin.z + 8)
                    .unwrap()
                    .name,
                expected
            );
        }
    }

    #[test]
    fn falling_lava_turns_water_below_into_stone() {
        let mut session = fluid_flow_session(FluidKind::Lava, false);
        let water = session.intern_liquid(FluidState {
            kind: FluidKind::Water,
            amount: 8,
            source: true,
            falling: false,
        });
        session.set(IVec3::new(8, 1, 8), water);
        assert_eq!(
            session
                .world
                .block(session.origin.x + 8, 2, session.origin.z + 8)
                .unwrap()
                .name,
            "minecraft:lava"
        );
        for _ in 0..31 {
            session.update(TICK);
        }
        assert_eq!(
            session
                .world
                .block(session.origin.x + 8, 1, session.origin.z + 8)
                .unwrap()
                .name,
            "minecraft:stone"
        );
        assert_eq!(
            session
                .world
                .block(session.origin.x + 8, 2, session.origin.z + 8)
                .unwrap()
                .name,
            "minecraft:lava"
        );
    }

    #[test]
    fn blue_ice_and_soul_soil_create_vertical_basalt() {
        let mut session = fluid_flow_session(FluidKind::Lava, false);
        let soul = session.intern_block(state("minecraft:soul_soil", &[]));
        let blue = session.intern_block(state("minecraft:blue_ice", &[]));
        session.set(IVec3::new(8, 1, 8), soul);
        session.set(IVec3::new(9, 2, 8), blue);
        let basalt = session
            .world
            .block(session.origin.x + 8, 2, session.origin.z + 8)
            .unwrap();
        assert_eq!(
            (basalt.name.as_str(), basalt.properties["axis"].as_str()),
            ("minecraft:basalt", "y")
        );
        assert_eq!(
            block_shape(basalt),
            Some(vec![Aabb {
                min: glam::Vec3::ZERO,
                max: glam::Vec3::ONE
            }])
        );
    }

    #[test]
    fn fast_lava_environment_overrides_dimension_name_for_spread() {
        for (named_nether, fast) in [(false, true), (true, false)] {
            let mut session = fluid_flow_session(FluidKind::Lava, named_nether);
            session.set_fast_lava(fast);
            let wall = IVec3::new(9, 2, 8);
            assert!(session.set(wall, session.states[0]));
            for _ in 0..11 {
                session.update(TICK);
            }
            assert_eq!(
                matches!(session.simulation_cell(wall), fluid_sim::Cell::Fluid(f) if f.kind == FluidKind::Lava),
                fast
            );
            if !fast {
                for _ in 0..20 {
                    session.update(TICK);
                }
                assert!(
                    matches!(session.simulation_cell(wall), fluid_sim::Cell::Fluid(f) if f.kind == FluidKind::Lava)
                );
            }
        }
    }

    #[test]
    fn shared_clocks_advance_on_fixed_ticks_and_respect_rules() {
        use native_persistence::world::{DimensionId, SavedClock};
        let id = DimensionId::parse("minecraft:overworld").unwrap();
        let mut session = session();
        let initial = SavedClock {
            total_ticks: 10,
            partial_tick: 0.0,
            rate: 0.5,
            paused: false,
        };
        session.set_world_clocks(BTreeMap::from([(id.clone(), initial)]), true);
        session.update(TICK / 2);
        assert_eq!(session.world_clock(&id), Some(initial));
        session.update(TICK / 2);
        assert_eq!(session.world_clock(&id).unwrap().partial_tick, 0.5);
        session.update(TICK);
        assert_eq!(session.world_clock(&id).unwrap().total_ticks, 11);
        let clock = session.world_clock(&id).unwrap();
        session.set_world_clocks(BTreeMap::from([(id.clone(), clock)]), false);
        session.update(TICK * 5);
        assert_eq!(session.world_clock(&id), Some(clock));
    }

    #[test]
    fn fast_lava_timeline_tracks_the_shared_clock_before_fluid_ticks() {
        use native_persistence::world::{DimensionId, SavedClock};
        use native_resources::{
            fluid_environment::load_fast_lava_environment, packs::ResourceProvider,
        };
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(match path {
                    "data/example/dimension_type/test.json" => Some(br#"{"timelines":["example:test"]}"#.to_vec()),
                    "data/example/timeline/test.json" => Some(br#"{"clock":"example:clock","period_ticks":10,"tracks":{"gameplay/fast_lava":{"keyframes":[{"ticks":0,"value":false},{"ticks":1,"value":true},{"ticks":3,"value":false}]}}}"#.to_vec()),
                    _ => None,
                })
            }
        }
        let environment = Arc::new(load_fast_lava_environment(&mut Data, "example:test").unwrap());
        let id = DimensionId::parse("example:clock").unwrap();
        let mut session = fluid_flow_session(FluidKind::Lava, false);
        session.set_world_clocks(BTreeMap::from([(id.clone(), SavedClock::default())]), true);
        session.set_fast_lava_environment(environment).unwrap();
        assert!(!session.fast_lava);
        let wall = IVec3::new(9, 2, 8);
        assert!(session.set(wall, session.states[0]));
        session.fluid_ticks.clear();
        session.fluid_pending.clear();
        session.schedule_fluid(
            IVec3::new(8, 2, 8),
            FluidState {
                kind: FluidKind::Lava,
                amount: 8,
                source: true,
                falling: false,
            },
            0,
        );
        session.update(TICK);
        assert!(session.fast_lava);
        assert!(
            matches!(session.simulation_cell(wall), fluid_sim::Cell::Fluid(f) if f.kind == FluidKind::Lava && f.amount == 7)
        );
        session.update(TICK);
        assert!(session.fast_lava);
        session.update(TICK);
        assert!(!session.fast_lava);
        let clock = session.world_clock(&id).unwrap();
        session.set_world_clocks(BTreeMap::from([(id, clock)]), false);
        session.update(TICK * 10);
        assert!(!session.fast_lava);
    }

    #[test]
    fn saved_source_conversion_rules_control_new_water_and_lava_sources() {
        for kind in [FluidKind::Water, FluidKind::Lava] {
            for enabled in [false, true] {
                let mut session = session();
                session.set_source_conversion(
                    kind == FluidKind::Water && enabled,
                    kind == FluidKind::Lava && enabled,
                );
                let source = FluidState {
                    kind,
                    amount: 8,
                    source: true,
                    falling: false,
                };
                let flowing = FluidState {
                    kind,
                    amount: 5,
                    source: false,
                    falling: false,
                };
                let source_id = session.intern_liquid(source);
                let flowing_id = session.intern_liquid(flowing);
                session.set(IVec3::new(7, 1, 8), source_id);
                session.set(IVec3::new(9, 1, 8), source_id);
                session.set(IVec3::new(8, 1, 8), flowing_id);
                session.fluid_ticks.clear();
                session.fluid_pending.clear();
                session.schedule_fluid(IVec3::new(8, 1, 8), flowing, 0);
                session.update(TICK);
                let fluid = match session.simulation_cell(IVec3::new(8, 1, 8)) {
                    fluid_sim::Cell::Fluid(fluid) => fluid,
                    other => panic!("expected fluid, got {other:?}"),
                };
                assert_eq!(fluid.source, enabled, "{kind:?}");
            }
        }
    }

    #[test]
    fn opening_a_wall_schedules_water_then_updates_shared_snapshots() {
        let mut session = fluid_flow_session(FluidKind::Water, false);
        let wall = IVec3::new(9, 2, 8);
        let before = session.snapshot();
        assert!(session.set(wall, session.states[0]));
        for _ in 0..5 {
            session.update(TICK);
        }
        assert_eq!(session.simulation_cell(wall), fluid_sim::Cell::Air);
        session.update(TICK);
        assert!(
            matches!(session.simulation_cell(wall), fluid_sim::Cell::Fluid(f) if f.kind == FluidKind::Water && f.amount == 7 && !f.falling)
        );
        let old_world = before.world.loaded().unwrap();
        assert_eq!(
            old_world
                .block(session.origin.x + 9, 2, session.origin.z + 8)
                .unwrap()
                .name,
            "minecraft:stone"
        );
        let new_world = session.snapshot();
        assert_eq!(
            new_world
                .world
                .loaded()
                .unwrap()
                .block(session.origin.x + 9, 2, session.origin.z + 8)
                .unwrap()
                .name,
            "minecraft:water"
        );
        for _ in 0..5 {
            session.update(TICK);
        }
        assert!(
            matches!(session.simulation_cell(IVec3::new(9, 1, 8)), fluid_sim::Cell::Fluid(f) if f.falling)
        );
    }

    #[test]
    fn falling_block_tick_precedes_fluid_tick_in_the_same_game_tick() {
        let mut session = fluid_flow_session(FluidKind::Water, false);
        session.set_falling_fire_tag(Arc::new(BTreeSet::new()));
        let sand_pos = IVec3::new(9, 2, 8);
        let water_pos = IVec3::new(8, 2, 8);
        let sand = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(sand_pos, sand));
        let fluid_sim::Cell::Fluid(water) = session.simulation_cell(water_pos) else {
            panic!("missing water source");
        };
        session.sand_ticks.clear();
        session.sand_pending.clear();
        session.fluid_ticks.clear();
        session.fluid_pending.clear();
        session.enqueue_saved_sand(&[SavedBlockTick {
            block: "minecraft:sand".into(),
            position: (session.origin + sand_pos).to_array(),
            delay: 0,
            priority: 0,
        }]);
        session.schedule_fluid(water_pos, water, 0);

        session.update(TICK);

        assert_eq!(session.falling_sand.len(), 1);
        assert!(matches!(
            session.simulation_cell(sand_pos),
            fluid_sim::Cell::Fluid(fluid) if fluid.kind == FluidKind::Water
        ));
    }

    #[test]
    fn removing_the_source_lets_flowing_water_recede() {
        let mut session = fluid_flow_session(FluidKind::Water, false);
        let wall = IVec3::new(9, 2, 8);
        session.set(wall, session.states[0]);
        for _ in 0..16 {
            session.update(TICK);
        }
        assert!(matches!(
            session.simulation_cell(wall),
            fluid_sim::Cell::Fluid(_)
        ));
        session.set(IVec3::new(8, 2, 8), session.states[0]);
        for _ in 0..180 {
            session.update(TICK);
        }
        assert_eq!(session.simulation_cell(wall), fluid_sim::Cell::Air);
        assert_eq!(
            session.simulation_cell(IVec3::new(9, 1, 8)),
            fluid_sim::Cell::Air
        );
    }

    #[test]
    fn lava_uses_dimension_dependent_delay_and_pending_ticks_are_deduplicated() {
        for (fast, delay) in [(false, 30), (true, 10)] {
            let mut session = fluid_flow_session(FluidKind::Lava, fast);
            let wall = IVec3::new(9, 2, 8);
            session.set(wall, session.states[0]);
            let source = match session.simulation_cell(IVec3::new(8, 2, 8)) {
                fluid_sim::Cell::Fluid(state) => state,
                _ => panic!("missing lava source"),
            };
            session.schedule_fluid(IVec3::new(8, 2, 8), source, 1);
            assert_eq!(session.fluid_pending.len(), 1);
            for _ in 0..delay {
                session.update(TICK);
            }
            assert_eq!(session.simulation_cell(wall), fluid_sim::Cell::Air);
            session.update(TICK);
            assert!(
                matches!(session.simulation_cell(wall), fluid_sim::Cell::Fluid(f) if f.kind == FluidKind::Lava)
            );
        }
    }

    #[test]
    fn scheduled_fluid_survives_eviction_and_origin_rebase() {
        let mut session = fluid_flow_session(FluidKind::Water, false);
        let wall = IVec3::new(9, 2, 8);
        let home = session.player_chunk().unwrap();
        let other = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        session.set(wall, session.states[0]);
        session.queue_chunk_window(other, 0);
        for _ in 0..10 {
            session.update(TICK);
        }
        assert!(!session.fluid_pending.is_empty());
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        assert!(
            matches!(session.simulation_cell(wall), fluid_sim::Cell::Fluid(f) if f.kind == FluidKind::Water)
        );
    }

    #[test]
    fn saved_fluid_ticks_keep_their_delay_priority_and_type() {
        let original = fluid_flow_session(FluidKind::Water, false);
        let origin = original.origin;
        let home = original.player_chunk().unwrap();
        let mut world = (*original.world).clone();
        world
            .set_block_id(origin.x + 9, 2, origin.z + 8, original.states[0])
            .unwrap();
        world.saved_fluid_ticks.insert(
            home,
            vec![
                SavedFluidTick {
                    fluid: "minecraft:water".into(),
                    position: [origin.x + 8, 2, origin.z + 8],
                    delay: 3,
                    priority: 1,
                },
                SavedFluidTick {
                    fluid: "minecraft:flowing_water".into(),
                    position: [origin.x + 9, 2, origin.z + 8],
                    delay: 3,
                    priority: -2,
                },
                SavedFluidTick {
                    fluid: "custom:unknown".into(),
                    position: [origin.x + 8, 2, origin.z + 8],
                    delay: 0,
                    priority: 0,
                },
            ],
        );
        let mut session =
            LoadedSession::new(world, origin, original.bounds, original.player).unwrap();
        let first = session.fluid_ticks.first_key_value().unwrap();
        assert_eq!((first.0.0, first.0.1, first.1.key.source), (3, -2, false));
        assert_eq!(session.take_stream_errors().len(), 1);
        for _ in 0..3 {
            session.update(TICK);
        }
        assert_eq!(
            session.simulation_cell(IVec3::new(9, 2, 8)),
            fluid_sim::Cell::Air
        );
        session.update(TICK);
        assert!(
            matches!(session.simulation_cell(IVec3::new(9, 2, 8)), fluid_sim::Cell::Fluid(f) if f.kind == FluidKind::Water)
        );
    }

    #[test]
    fn saved_duplicate_fluid_ticks_are_retained_but_new_duplicates_are_rejected() {
        let mut session = session();
        let pos = session.origin + IVec3::new(8, 2, 6);
        let chunk = session.player_chunk().unwrap();
        let tick = SavedFluidTick {
            fluid: "minecraft:water".into(),
            position: pos.to_array(),
            delay: 3,
            priority: 0,
        };
        session.enqueue_saved_fluids(chunk, &[tick.clone(), tick]);
        assert_eq!(session.fluid_ticks.len(), 2);
        assert_eq!(session.fluid_pending.len(), 1);
        let first = *session.fluid_ticks.first_key_value().unwrap().1;
        session.insert_fluid_tick(first.key, 1, -1, 0);
        assert_eq!(session.fluid_ticks.len(), 2);
        let order: Vec<_> = session.fluid_ticks.keys().map(|key| key.2).collect();
        assert_eq!(order, vec![-2, -1]);
    }

    #[test]
    fn sand_ticks_keep_saved_order_and_deduplicate_new_notifications() {
        let mut session = session();
        let relative = IVec3::new(8, 2, 6);
        let absolute = session.origin + relative;
        let saved = SavedBlockTick {
            block: "minecraft:sand".into(),
            position: absolute.to_array(),
            delay: 7,
            priority: -1,
        };
        session.enqueue_saved_sand(&[saved.clone(), saved]);
        let sand = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(relative, sand));
        assert_eq!(session.sand_ticks.len(), 2);
        assert_eq!(session.sand_pending.len(), 1);
        assert_eq!(
            session.sand_ticks.keys().map(|v| v.2).collect::<Vec<_>>(),
            vec![-2, -1]
        );

        let other = IVec3::new(9, 2, 6);
        let red = session.intern_block(state("minecraft:red_sand", &[]));
        assert!(session.set(other, red));
        let due: Vec<_> = session
            .sand_ticks
            .iter()
            .filter(|(_, key)| key.kind == FallingKind::RedSand)
            .map(|(order, _)| order.0)
            .collect();
        assert_eq!(due, vec![2]);
        assert!(session.set(other + IVec3::Y, session.states[3]));
        assert_eq!(
            session
                .sand_ticks
                .iter()
                .filter(|(_, key)| key.kind == FallingKind::RedSand)
                .count(),
            1
        );
    }

    #[test]
    fn saved_duplicate_sand_tick_allows_reschedule_after_first_poll() {
        let mut session = session();
        let relative = IVec3::new(8, 2, 6);
        let absolute = session.origin + relative;
        let sand = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(relative + IVec3::NEG_Y, session.states[3]));
        assert!(session.set(relative, sand));
        session.sand_ticks.clear();
        session.sand_pending.clear();
        let saved = |delay| SavedBlockTick {
            block: "minecraft:sand".into(),
            position: absolute.to_array(),
            delay,
            priority: 0,
        };
        session.enqueue_saved_sand(&[saved(0), saved(7)]);
        session.advance_sand_ticks();
        assert_eq!(session.sand_ticks.len(), 1);
        assert!(session.sand_pending.is_empty());
        session.schedule_sand(relative, FallingKind::Sand);
        assert_eq!(
            session
                .sand_ticks
                .keys()
                .map(|order| order.0)
                .collect::<Vec<_>>(),
            vec![2, 7]
        );
        assert_eq!(session.sand_pending.len(), 1);
    }

    #[test]
    fn falling_tick_draft_includes_deferred_ticks_and_rejects_unknown_types() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let pos = session.origin + IVec3::new(8, 5, 8);
        session.tick = 5;
        let first = SandTickKey {
            x: pos.x,
            y: pos.y,
            z: pos.z,
            kind: FallingKind::Sand,
        };
        let second = SandTickKey {
            x: pos.x + 1,
            y: pos.y,
            z: pos.z,
            kind: FallingKind::Gravel,
        };
        session.sand_deferred.insert((3, 0, 1, 0), first);
        session.sand_ticks.insert((8, -1, -1, 1), second);
        let draft = session.draft_falling_block_ticks(home).unwrap();
        assert_eq!(draft.len(), 2);
        assert_eq!(
            (&draft[0].block[..], draft[0].delay, draft[0].priority),
            ("minecraft:gravel", 3, -1)
        );
        assert_eq!(
            (&draft[1].block[..], draft[1].delay, draft[1].priority),
            ("minecraft:sand", -2, 0)
        );
        Arc::make_mut(&mut session.world).saved_block_ticks.insert(
            home,
            vec![SavedBlockTick {
                block: "minecraft:observer".into(),
                position: pos.to_array(),
                delay: 3,
                priority: 0,
            }],
        );
        assert!(session.draft_falling_block_ticks(home).is_err());
    }

    #[test]
    fn mixed_block_tick_draft_keeps_saved_subtick_order() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        session.tick = 5;
        let position = (session.origin + IVec3::new(8, 5, 8)).to_array();
        let source_ticks = vec![
            SavedBlockTick {
                block: "minecraft:stone_button".into(),
                position,
                delay: 7,
                priority: 0,
            },
            SavedBlockTick {
                block: "minecraft:sand".into(),
                position,
                delay: 2,
                priority: -1,
            },
            SavedBlockTick {
                block: "minecraft:dirt_path".into(),
                position,
                delay: -1,
                priority: 0,
            },
            SavedBlockTick {
                block: "minecraft:farmland".into(),
                position,
                delay: 5,
                priority: 1,
            },
            SavedBlockTick {
                block: "minecraft:gravel".into(),
                position,
                delay: 3,
                priority: 1,
            },
        ];
        Arc::make_mut(&mut session.world)
            .saved_block_ticks
            .insert(home, source_ticks.clone());
        session.enqueue_saved_sand(&source_ticks);
        session.enqueue_saved_buttons(&source_ticks);
        session.enqueue_saved_soil(&source_ticks);

        let drafted = session.draft_block_ticks(home).unwrap();
        assert_eq!(drafted, source_ticks);
    }

    #[test]
    fn falling_and_button_ticks_share_the_per_tick_work_limit() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let button_relative = IVec3::new(8, 2, 7);
        let button_absolute = session.origin + button_relative;
        let mut button = stone_button_state(IVec3::Z, 0.0).unwrap();
        button.properties.insert("powered".into(), "true".into());
        let button_id = session.intern_block(button);
        assert!(session.set(button_relative, button_id));

        let saved_sand = SavedBlockTick {
            block: "minecraft:sand".into(),
            position: (session.origin + IVec3::new(9, 2, 7)).to_array(),
            delay: 0,
            priority: 0,
        };
        session.enqueue_saved_sand(&vec![saved_sand; 65_536]);
        session.enqueue_saved_buttons(&[SavedBlockTick {
            block: "minecraft:stone_button".into(),
            position: button_absolute.to_array(),
            delay: 0,
            priority: 0,
        }]);

        session.advance_block_ticks();
        assert_eq!(session.sand_ticks.len(), 0);
        assert_eq!(session.button_ticks.len(), 1);
        assert!(session.button_pending.contains(&(
            button_absolute.x,
            button_absolute.y,
            button_absolute.z
        )));
        assert_eq!(
            session
                .world
                .block(button_absolute.x, button_absolute.y, button_absolute.z)
                .unwrap()
                .properties["powered"],
            "true"
        );

        session.advance_block_ticks();
        assert!(session.button_ticks.is_empty());
        assert_eq!(
            session
                .world
                .block(button_absolute.x, button_absolute.y, button_absolute.z)
                .unwrap()
                .properties["powered"],
            "false"
        );
        assert!(session.draft_block_ticks(home).unwrap().is_empty());
    }

    #[test]
    fn saved_cross_kind_order_controls_falling_support_and_button_release() {
        fn run(button_first: bool) -> Vec<ButtonSoundEvent> {
            let mut session = session();
            let button_relative = IVec3::new(8, 2, 7);
            let sand_relative = IVec3::new(9, 2, 7);
            let mut button = stone_button_state(IVec3::NEG_X, 0.0).unwrap();
            button.properties.insert("powered".into(), "true".into());
            let button_id = session.intern_block(button);
            let sand_id = session.intern_block(state("minecraft:sand", &[]));
            assert!(session.set(sand_relative, sand_id));
            assert!(session.set(button_relative, button_id));
            session.set_falling_fire_tag(Arc::new(BTreeSet::new()));

            // Remove schedules caused by placing the support and button. The
            // source list below models their saved order in one Java chunk.
            session.sand_ticks.clear();
            session.sand_pending.clear();
            session.button_ticks.clear();
            session.button_pending.clear();
            let button_tick = SavedBlockTick {
                block: "minecraft:stone_button".into(),
                position: (session.origin + button_relative).to_array(),
                delay: 0,
                priority: 0,
            };
            let sand_tick = SavedBlockTick {
                block: "minecraft:sand".into(),
                position: (session.origin + sand_relative).to_array(),
                delay: 0,
                priority: 0,
            };
            let saved = if button_first {
                vec![button_tick, sand_tick]
            } else {
                vec![sand_tick, button_tick]
            };
            session.enqueue_saved_sand(&saved);
            session.enqueue_saved_buttons(&saved);
            session.advance_block_ticks();
            session.take_button_sounds()
        }

        let button_first = run(true);
        assert_eq!(button_first.len(), 1);
        assert!(!button_first[0].powered);
        assert_eq!(button_first[0].tick, 0);
        assert!(run(false).is_empty());
    }

    #[test]
    fn falling_tick_draft_rejects_unloaded_chunks_and_unrepresentable_delays() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let absent = ChunkPos {
            x: home.x + 9,
            z: home.z,
        };
        assert!(session.draft_falling_block_ticks(absent).is_err());
        let pos = session.origin + IVec3::new(8, 5, 8);
        let key = SandTickKey {
            x: pos.x,
            y: pos.y,
            z: pos.z,
            kind: FallingKind::Sand,
        };
        session
            .sand_ticks
            .insert((i128::from(i32::MAX) + 1, 0, 0, 0), key);
        assert!(session.draft_falling_block_ticks(home).is_err());
    }

    #[test]
    fn fluid_tick_draft_keeps_type_order_and_rejects_unknown_source() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let pos = session.origin + IVec3::new(8, 5, 8);
        session.tick = 5;
        let water = FluidTickKey {
            x: pos.x,
            y: pos.y,
            z: pos.z,
            kind: FluidKind::Water,
            source: true,
        };
        let lava = FluidTickKey {
            x: pos.x + 1,
            y: pos.y,
            z: pos.z,
            kind: FluidKind::Lava,
            source: false,
        };
        session
            .fluid_ticks
            .insert((3, 0, 1, 0), ScheduledFluid { key: water });
        session
            .fluid_ticks
            .insert((8, -1, -1, 1), ScheduledFluid { key: lava });
        let ticks = session.draft_fluid_ticks(home).unwrap();
        assert_eq!(ticks.len(), 2);
        assert_eq!(
            (&ticks[0].fluid[..], ticks[0].delay, ticks[0].priority),
            ("minecraft:flowing_lava", 3, -1)
        );
        assert_eq!(
            (&ticks[1].fluid[..], ticks[1].delay, ticks[1].priority),
            ("minecraft:water", -2, 0)
        );
        Arc::make_mut(&mut session.world).saved_fluid_ticks.insert(
            home,
            vec![SavedFluidTick {
                fluid: "example:fluid".into(),
                position: pos.to_array(),
                delay: 3,
                priority: 0,
            }],
        );
        assert!(session.draft_fluid_ticks(home).is_err());
    }

    #[test]
    fn scheduled_sand_falls_through_air_and_lands_on_stone() {
        let mut session = session();
        let source = IVec3::new(10, 5, 8);
        let id = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(source, id));
        session.update(TICK);
        let before = session.snapshot();
        session.update(TICK);
        assert!(session.snapshot().falling_blocks.is_empty());
        session.update(TICK);
        let snapshot = session.snapshot();
        assert_eq!(snapshot.falling_blocks.len(), 1);
        assert!(before.falling_blocks.is_empty());
        let absolute = session.origin + source;
        assert_eq!(
            before
                .world
                .loaded()
                .unwrap()
                .block(absolute.x, absolute.y, absolute.z)
                .unwrap()
                .name,
            "minecraft:sand"
        );
        assert!(
            snapshot.falling_blocks[0].position.y < snapshot.falling_blocks[0].previous_position.y
        );
        assert!(
            matches!(cell(&session.world,session.origin,session.bounds,source,session.states[0]), Cell::State(value) if value == session.states[0])
        );
        for _ in 0..40 {
            session.update(TICK);
        }
        assert!(session.snapshot().falling_blocks.is_empty());
        assert!(
            matches!(cell(&session.world,session.origin,session.bounds,IVec3::new(10,1,8),session.states[0]), Cell::State(value) if value == id)
        );
    }

    #[test]
    fn saved_gravel_tick_falls_and_lands_as_gravel() {
        let mut session = session();
        let source = IVec3::new(10, 5, 8);
        let gravel = session.intern_block(state("minecraft:gravel", &[]));
        assert!(session.set(source, gravel));
        session.sand_ticks.clear();
        session.sand_pending.clear();
        let absolute = session.origin + source;
        session.enqueue_saved_sand(&[SavedBlockTick {
            block: "minecraft:gravel".into(),
            position: absolute.to_array(),
            delay: 0,
            priority: 0,
        }]);
        session.advance_sand_ticks();
        assert_eq!(session.falling_sand.len(), 1);
        assert_eq!(session.falling_sand_snapshot()[0].kind, FallingKind::Gravel);
        for _ in 0..40 {
            session.advance_falling_sand();
        }
        assert!(session.falling_sand.is_empty());
        assert!(
            matches!(cell(&session.world, session.origin, session.bounds, IVec3::new(10, 1, 8), session.states[0]), Cell::State(id) if id == gravel)
        );
    }

    #[test]
    fn gravel_reacts_to_removed_support_after_two_ticks() {
        let mut session = session();
        let source = IVec3::new(10, 5, 8);
        let gravel = session.intern_block(state("minecraft:gravel", &[]));
        assert!(session.set(source + IVec3::NEG_Y, session.states[3]));
        assert!(session.set(source, gravel));
        for _ in 0..3 {
            session.update(TICK);
        }
        assert!(session.falling_sand.is_empty());
        assert!(session.set(source + IVec3::NEG_Y, session.states[0]));
        session.update(TICK);
        session.update(TICK);
        assert!(session.falling_sand.is_empty());
        session.update(TICK);
        assert_eq!(session.falling_sand.len(), 1);
        assert_eq!(session.falling_sand[0].kind, FallingKind::Gravel);
    }

    #[test]
    fn removed_sand_does_not_spawn_when_its_old_tick_is_due() {
        let mut session = session();
        let source = IVec3::new(10, 5, 8);
        let sand = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(source, sand));
        assert!(session.set(source, session.states[0]));
        for _ in 0..5 {
            session.update(TICK);
        }
        assert!(session.snapshot().falling_blocks.is_empty());
        assert!(session.sand_ticks.is_empty());
    }

    #[test]
    fn sand_waits_for_fire_tag_and_uses_world_override() {
        let mut session = session();
        let source = IVec3::new(10, 5, 8);
        assert!(session.set(source + IVec3::NEG_Y, session.states[3]));
        let sand = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(source, sand));
        session.falling_fire_tag = None;
        session.tick = 2;
        session.advance_sand_ticks();
        assert!(session.falling_sand.is_empty());
        assert_eq!(session.sand_deferred.len(), 1);
        session.set_falling_fire_tag(Arc::new(BTreeSet::from(["minecraft:stone".into()])));
        session.advance_sand_ticks();
        assert_eq!(session.falling_sand.len(), 1);
    }

    #[test]
    fn sand_tick_waits_when_its_chunk_is_not_loaded() {
        let mut session = session();
        let key = SandTickKey {
            x: session.origin.x + 10_000,
            y: session.origin.y + 5,
            z: session.origin.z + 10_000,
            kind: FallingKind::Sand,
        };
        let order = (0, 0, 0, 0);
        session.sand_pending.insert(key);
        session.sand_ticks.insert(order, key);
        session.advance_sand_ticks();
        assert_eq!(session.sand_deferred.get(&order), Some(&key));
        assert!(session.sand_pending.contains(&key));
        assert!(session.falling_sand.is_empty());
    }

    #[test]
    fn deferred_sand_retries_when_its_chunk_streams_in() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let neighbor = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let key = SandTickKey {
            x: neighbor.x * 16 + 8,
            y: 5,
            z: neighbor.z * 16 + 8,
            kind: FallingKind::Sand,
        };
        let order = (0, 0, 0, 0);
        session.sand_pending.insert(key);
        session.sand_ticks.insert(order, key);
        session.advance_sand_ticks();
        assert_eq!(session.sand_deferred.get(&order), Some(&key));

        let mut incoming = World::new("minecraft:overworld");
        let air = incoming.intern_state(state("minecraft:air", &[]));
        let sand = incoming.intern_state(state("minecraft:sand", &[]));
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        blocks[8 + 8 * 16 + 5 * 256] = sand;
        incoming.chunks.insert(
            neighbor,
            Arc::new(Chunk {
                position: neighbor,
                status: Some("minecraft:full".into()),
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
        session.queue_chunk_window(home, 1);
        session.queue_loaded_chunk(neighbor, incoming);
        session.update(TICK);
        assert!(session.sand_deferred.is_empty());
        assert!(!session.sand_pending.contains(&key));
        assert_eq!(session.falling_sand.len(), 1);
        assert!(
            session
                .world
                .block(key.x, key.y, key.z)
                .is_some_and(|state| state.name == "minecraft:air")
        );
    }

    #[test]
    fn deferred_sand_retries_after_world_change_but_does_not_block_other_ticks() {
        let mut session = session();
        let source = IVec3::new(10, 5, 8);
        let unknown = session.intern_block(state("example:unknown", &[]));
        let sand = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(source + IVec3::NEG_Y, unknown));
        assert!(session.set(source, sand));
        session.tick = 2;
        session.advance_sand_ticks();
        assert_eq!(session.sand_deferred.len(), 1);
        for _ in 0..5 {
            session.advance_sand_ticks();
        }
        assert_eq!(session.sand_deferred.len(), 1);
        assert!(session.sand_ticks.is_empty());
        let other = IVec3::new(11, 5, 8);
        assert!(session.set(other + IVec3::NEG_Y, session.states[3]));
        assert!(session.set(other, sand));
        session.tick = 4;
        session.advance_sand_ticks();
        assert_eq!(session.sand_deferred.len(), 1);
        assert!(session.sand_ticks.is_empty());
        assert!(session.set(source + IVec3::NEG_Y, session.states[3]));
        session.advance_sand_ticks();
        assert!(session.sand_deferred.is_empty());
        assert!(session.sand_pending.is_empty());
    }

    #[test]
    fn deferred_sand_observes_world_edits_made_later_in_the_same_pass() {
        let mut session = session();
        let unknown = session.intern_block(state("example:unknown", &[]));
        let sand = session.intern_block(state("minecraft:sand", &[]));
        let first = IVec3::new(8, 5, 8);
        let second = IVec3::new(10, 5, 8);
        assert!(session.set(first + IVec3::NEG_Y, unknown));
        assert!(session.set(first, sand));
        assert!(session.set(second, sand));
        session.tick = 2;
        session.advance_sand_ticks();
        assert_eq!(session.sand_deferred.len(), 1);
        assert_eq!(session.falling_sand.len(), 1);
        assert_ne!(session.sand_deferred_revision, Some(session.revision));
        session.advance_sand_ticks();
        assert_eq!(session.sand_deferred.len(), 1);
        assert_eq!(session.sand_deferred_revision, Some(session.revision));
    }

    #[test]
    fn sand_tick_finishes_on_known_full_cube_support() {
        let mut session = session();
        let source = IVec3::new(10, 5, 8);
        let granite = session.intern_block(state("minecraft:granite", &[]));
        let sand = session.intern_block(state("minecraft:sand", &[]));
        assert!(session.set(source + IVec3::NEG_Y, granite));
        assert!(session.set(source, sand));
        session.tick = 2;
        session.advance_sand_ticks();
        assert!(session.sand_ticks.is_empty());
        assert!(session.sand_pending.is_empty());
        assert!(session.falling_sand.is_empty());
        assert!(
            matches!(cell(&session.world, session.origin, session.bounds, source, session.states[0]), Cell::State(id) if id == sand)
        );
    }

    #[test]
    fn sand_tick_finishes_on_known_partial_collision_support() {
        for support in [
            state("minecraft:farmland", &[("moisture", "0")]),
            state(
                "minecraft:oak_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            ),
            state("minecraft:flower_pot", &[]),
        ] {
            let mut session = session();
            let source = IVec3::new(10, 5, 8);
            let below = session.intern_block(support);
            let sand = session.intern_block(state("minecraft:sand", &[]));
            assert!(session.set(source + IVec3::NEG_Y, below));
            assert!(session.set(source, sand));
            session.tick = 2;
            session.advance_sand_ticks();
            assert!(session.sand_ticks.is_empty());
            assert!(session.falling_sand.is_empty());
        }
    }

    #[test]
    fn sand_tick_finishes_on_known_nonreplaceable_empty_shapes() {
        for name in [
            "minecraft:dandelion",
            "minecraft:torch",
            "minecraft:oak_sapling",
        ] {
            let mut session = session();
            let source = IVec3::new(10, 5, 8);
            let below = session.intern_block(state(name, &[]));
            let sand = session.intern_block(state("minecraft:sand", &[]));
            assert!(session.set(source + IVec3::NEG_Y, below));
            assert!(session.set(source, sand));
            session.tick = 2;
            session.advance_sand_ticks();
            assert!(session.sand_ticks.is_empty(), "{name}");
            assert!(session.falling_sand.is_empty(), "{name}");
        }
    }

    #[test]
    fn falling_sand_expires_at_java_age_boundaries_and_respects_entity_drops() {
        for (outside, age, drops) in [(true, 99, true), (true, 99, false), (false, 599, true)] {
            let mut session = session();
            session.set_entity_drops(drops);
            let relative_y = if outside {
                session.bounds.max_y - session.origin.y + 2
            } else {
                5
            };
            let position = (session.origin + IVec3::new(10, relative_y, 8)).as_dvec3()
                + DVec3::new(0.5, 0.0, 0.5);
            session.falling_sand.push(FallingSand {
                previous: position,
                motion: crate::falling_block::AirborneMotion {
                    position,
                    velocity: DVec3::ZERO,
                    age,
                    no_gravity: true,
                },
                kind: FallingKind::RedSand,
                drop_item: true,
                cancel_drop: false,
                on_ground: false,
                source: None,
            });
            session.advance_falling_sand();
            assert_eq!(session.falling_sand.len(), 1);
            assert!(session.dropped_items.snapshot().is_empty());
            session.advance_falling_sand();
            assert!(session.falling_sand.is_empty());
            let items = session.dropped_items.snapshot();
            assert_eq!(items.len(), usize::from(drops));
            if drops {
                assert_eq!(items[0].position, position);
                assert_eq!(items[0].stack.id(), "minecraft:red_sand");
            }
        }
    }

    #[test]
    fn sand_landing_inside_slab_drops_item_only_when_enabled() {
        for drops in [false, true] {
            let mut session = session();
            session.set_entity_drops(drops);
            let slab = session.intern_block(state(
                "minecraft:oak_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            ));
            assert!(session.set(IVec3::new(10, 1, 8), slab));
            let position =
                (session.origin + IVec3::new(10, 4, 8)).as_dvec3() + DVec3::new(0.5, 0.0, 0.5);
            session.falling_sand.push(FallingSand {
                previous: position,
                motion: crate::falling_block::AirborneMotion::from_block(
                    session.origin + IVec3::new(10, 4, 8),
                ),
                kind: FallingKind::Sand,
                drop_item: true,
                cancel_drop: false,
                on_ground: false,
                source: None,
            });
            for _ in 0..40 {
                session.advance_falling_sand();
            }
            assert!(session.falling_sand.is_empty());
            let items = session.dropped_items.snapshot();
            assert_eq!(items.len(), usize::from(drops));
            assert!(
                matches!(cell(&session.world, session.origin, session.bounds, IVec3::new(10, 1, 8), session.states[0]), Cell::State(id) if id == slab)
            );
            if drops {
                assert_eq!(items[0].stack.id(), "minecraft:sand");
            }
        }
    }

    #[test]
    fn sand_landing_on_neighbor_edge_without_support_drops_instead_of_placing() {
        for drops in [false, true] {
            let mut session = session();
            session.set_entity_drops(drops);
            assert!(session.set(IVec3::new(10, 0, 8), session.states[0]));
            let position =
                (session.origin + IVec3::new(10, 4, 8)).as_dvec3() + DVec3::new(0.1, 0.0, 0.5);
            session.falling_sand.push(FallingSand {
                previous: position,
                motion: crate::falling_block::AirborneMotion {
                    position,
                    velocity: DVec3::ZERO,
                    age: 0,
                    no_gravity: false,
                },
                kind: FallingKind::Sand,
                drop_item: true,
                cancel_drop: false,
                on_ground: false,
                source: None,
            });
            for _ in 0..40 {
                session.advance_falling_sand();
            }
            assert!(session.falling_sand.is_empty());
            assert_eq!(session.dropped_items.snapshot().len(), usize::from(drops));
            assert!(
                matches!(cell(&session.world, session.origin, session.bounds, IVec3::new(10, 1, 8), session.states[0]), Cell::State(id) if id == session.states[0])
            );
        }
    }

    #[test]
    fn sand_falls_through_fire_and_replaces_fire_at_landing() {
        for fire_name in ["minecraft:fire", "minecraft:soul_fire"] {
            let mut session = session();
            session.set_falling_fire_tag(Arc::new(BTreeSet::from([fire_name.into()])));
            let fire = session.intern_block(state(fire_name, &[]));
            assert!(session.set(IVec3::new(10, 1, 8), fire));
            assert!(session.set(IVec3::new(10, 3, 8), fire));
            let sand = session.intern_block(state("minecraft:sand", &[]));
            assert!(session.set(IVec3::new(10, 4, 8), sand));
            for _ in 0..50 {
                session.update(TICK);
            }
            assert!(session.falling_sand.is_empty());
            assert!(matches!(
                cell(&session.world, session.origin, session.bounds, IVec3::new(10, 1, 8), session.states[0]),
                Cell::State(id) if id == sand
            ));
        }
    }

    #[test]
    fn sand_falls_through_replaceable_plants() {
        for plant_name in [
            "minecraft:short_grass",
            "minecraft:fern",
            "minecraft:dead_bush",
        ] {
            let mut session = session();
            let plant = session.intern_block(state(plant_name, &[]));
            assert!(session.set(IVec3::new(10, 1, 8), plant));
            assert!(session.set(IVec3::new(10, 3, 8), plant));
            let sand = session.intern_block(state("minecraft:sand", &[]));
            assert!(session.set(IVec3::new(10, 4, 8), sand));
            for _ in 0..50 {
                session.update(TICK);
            }
            assert!(session.falling_sand.is_empty(), "{plant_name}");
            assert!(
                matches!(
                    cell(&session.world, session.origin, session.bounds, IVec3::new(10, 1, 8), session.states[0]),
                    Cell::State(id) if id == sand
                ),
                "{plant_name}"
            );
        }
    }

    #[test]
    fn sand_falls_through_ordinary_liquids_and_replaces_them_on_landing() {
        for liquid in ["minecraft:water", "minecraft:lava"] {
            for level in ["0", "7", "8", "15"] {
                let mut session = session();
                let fluid = session.intern_block(state(liquid, &[("level", level)]));
                for y in 1..=4 {
                    assert!(session.set(IVec3::new(10, y, 8), fluid));
                }
                let sand = session.intern_block(state("minecraft:sand", &[]));
                assert!(session.set(IVec3::new(10, 5, 8), sand));
                session.tick = 2;
                session.advance_sand_ticks();
                assert_eq!(session.falling_sand.len(), 1, "{liquid} level {level}");
                for _ in 0..40 {
                    session.advance_falling_sand();
                }
                assert!(session.falling_sand.is_empty(), "{liquid} level {level}");
                assert!(
                    matches!(cell(&session.world, session.origin, session.bounds, IVec3::new(10, 1, 8), session.states[0]), Cell::State(id) if id == sand),
                    "{liquid} level {level}"
                );
            }
        }
    }

    #[test]
    fn streamed_saved_fluid_ticks_are_loaded_only_once_per_chunk() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let neighbor = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let mut incoming = World::new("minecraft:overworld");
        let stone = incoming.intern_state(state("minecraft:stone", &[]));
        let water = incoming.intern_state(state("minecraft:water", &[("level", "0")]));
        let mut blocks = Box::new([stone; BLOCK_COUNT]);
        blocks[8 + 8 * 16 + 2 * 256] = water;
        incoming.chunks.insert(
            neighbor,
            Arc::new(Chunk {
                position: neighbor,
                status: Some("minecraft:full".into()),
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
        incoming.saved_fluid_ticks.insert(
            neighbor,
            vec![SavedFluidTick {
                fluid: "minecraft:water".into(),
                position: [neighbor.x * 16 + 8, 2, neighbor.z * 16 + 8],
                delay: 1,
                priority: 0,
            }],
        );
        incoming.saved_block_ticks.insert(
            neighbor,
            vec![SavedBlockTick {
                block: "minecraft:sand".into(),
                position: [neighbor.x * 16 + 7, 2, neighbor.z * 16 + 8],
                delay: 100,
                priority: 0,
            }],
        );
        session.queue_chunk_window(home, 1);
        session.queue_loaded_chunk(neighbor, incoming.clone());
        session.update(TICK);
        assert_eq!(session.fluid_pending.len(), 1);
        assert_eq!(session.sand_ticks.len(), 1);
        session.update(TICK);
        assert!(session.fluid_pending.is_empty());
        assert_eq!(session.sand_ticks.len(), 1);
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        assert!(!session.world.chunks.contains_key(&neighbor));
        session.queue_chunk_window(home, 1);
        session.queue_loaded_chunk(neighbor, incoming);
        session.update(TICK);
        assert!(session.world.chunks.contains_key(&neighbor));
        assert!(session.fluid_pending.is_empty());
        assert_eq!(session.sand_ticks.len(), 1);
    }

    #[test]
    fn liquid_controls_apply_at_fixed_ticks() {
        let mut rising = liquid_session("minecraft:water");
        let mut sinking = liquid_session("minecraft:water");
        rising.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: true,
        });
        sinking.submit_command(PlayerCommand::Sneak(true));
        for _ in 0..10 {
            rising.update(TICK);
            sinking.update(TICK);
        }
        assert!(
            rising.motion.position.y > 2.5,
            "{rising:?}",
            rising = rising.motion
        );
        assert!(
            sinking.motion.position.y < 1.8,
            "{sinking:?}",
            sinking = sinking.motion
        );
        assert_eq!(sinking.snapshot().player.pose, PlayerPose::Crouching);

        let mut lava = liquid_session("minecraft:lava");
        lava.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: true,
        });
        for _ in 0..10 {
            lava.update(TICK);
        }
        assert!(lava.motion.position.y < rising.motion.position.y);
        assert!(lava.motion.position.y >= 1.0);
    }

    #[test]
    fn submerged_sprint_selects_and_leaves_swimming_pose() {
        let mut session = liquid_session("minecraft:water");
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: false,
        });
        session.submit_command(PlayerCommand::Sprint {
            pressed: true,
            backward_pressed: false,
        });
        for _ in 0..5 {
            session.update(TICK);
        }
        assert!(session.sprint.active);
        assert!(session.swimming);
        assert_eq!(session.snapshot().player.pose, PlayerPose::Swimming);
        assert!(session.snapshot().player.position.z < 8.5);
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: false,
        });
        for _ in 0..3 {
            session.update(TICK);
        }
        assert!(!session.swimming);
        assert_eq!(session.snapshot().player.pose, PlayerPose::Standing);
    }

    #[test]
    fn swimming_is_independent_of_frame_cadence() {
        let mut frequent = liquid_session("minecraft:water");
        let mut ticks = liquid_session("minecraft:water");
        for session in [&mut frequent, &mut ticks] {
            session.submit_command(PlayerCommand::Move {
                forward: 1.0,
                strafe: 0.0,
                jump: true,
            });
            session.submit_command(PlayerCommand::Sprint {
                pressed: true,
                backward_pressed: false,
            });
        }
        for _ in 0..100 {
            frequent.update(Duration::from_millis(10));
        }
        for _ in 0..20 {
            ticks.update(TICK);
        }
        assert_eq!(frequent.snapshot().tick, 20);
        assert_eq!(
            frequent.snapshot().player.position,
            ticks.snapshot().player.position
        );
        assert_eq!(
            frequent.snapshot().player.pose,
            ticks.snapshot().player.pose
        );
        assert_eq!(frequent.snapshot().sprinting, ticks.snapshot().sprinting);
        assert_eq!(frequent.motion.velocity, ticks.motion.velocity);
    }

    #[test]
    fn flowing_water_pushes_player_and_missing_neighbor_disables_sampling() {
        let mut session = liquid_session("minecraft:water");
        let flowing = Arc::make_mut(&mut session.world)
            .intern_state(state("minecraft:water", &[("level", "7")]));
        session
            .shapes
            .push(block_shape(session.world.state(flowing).unwrap()));
        session
            .outlines
            .push(block_outline(session.world.state(flowing).unwrap()));
        session
            .fluids
            .push(FluidState::from_block(session.world.state(flowing).unwrap()).unwrap());
        let home = session.player_chunk().unwrap();
        let world = Arc::make_mut(&mut session.world);
        let chunk = Arc::make_mut(world.chunks.get_mut(&home).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        let blocks = section.blocks.as_mut().unwrap();
        for y in 1..6 {
            blocks[9 + 8 * 16 + y * 256] = flowing;
        }
        session.update(TICK);
        assert!(session.motion.position.x > 8.5);

        let view = FluidView {
            world: &session.world,
            shapes: &session.shapes,
            fluids: &session.fluids,
            origin: session.origin,
            bounds: session.bounds,
            air: session.states[0],
        };
        assert!(view.chunks_loaded(crate::movement::player_bounds(
            DVec3::new(8.5, 2.0, 8.5),
            PLAYER_HEIGHT
        )));
        assert!(!view.chunks_loaded(crate::movement::player_bounds(
            DVec3::new(0.5, 2.0, 8.5),
            PLAYER_HEIGHT
        )));
    }

    #[test]
    fn streamed_chunks_apply_on_ticks_and_preserve_dirty_chunks_across_eviction() {
        let mut session = session();
        let origin = session.origin;
        let home = ChunkPos {
            x: origin.x.div_euclid(16),
            z: origin.z.div_euclid(16),
        };
        let neighbor = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        session
            .set_initial_entity_chunk(home, SavedEntityData::Empty)
            .unwrap();
        let mut incoming = World::new("minecraft:overworld");
        let ice = incoming.intern_state(state("minecraft:ice", &[]));
        let biome = incoming.intern_biome("minecraft:forest".into());
        incoming.chunks.insert(
            neighbor,
            Arc::new(Chunk {
                position: neighbor,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(Box::new([ice; BLOCK_COUNT])),
                        biomes: Some(Box::new([biome; native_world::loaded::BIOME_COUNT])),
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let before = session.snapshot();
        session.queue_chunk_window(home, 1);
        session.queue_loaded_chunk_with_entities(neighbor, incoming, SavedEntityData::Empty);
        assert!(session.has_chunk_or_pending(neighbor));
        session.update(Duration::from_millis(49));
        assert!(session.saved_entity_chunk(neighbor).is_none());
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&neighbor)
        );
        session.update(Duration::from_millis(1));
        let loaded = session.snapshot();
        assert!(matches!(
            session.saved_entity_chunk(neighbor),
            Some(SavedEntityData::Empty)
        ));
        assert!(
            !before
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&neighbor)
        );
        let world = loaded.world.loaded().unwrap();
        assert_eq!(
            world.block(origin.x + 16, 0, origin.z).unwrap().name,
            "minecraft:ice"
        );
        assert_eq!(
            world.biome(origin.x + 16, 0, origin.z),
            Some("minecraft:forest")
        );
        assert_eq!(session.shapes.len(), world.state_count());
        assert_eq!(session.outlines.len(), world.state_count());
        assert_eq!(session.fluids.len(), world.state_count());

        let target = IVec3::new(8, 2, 6);
        assert!(session.set(target, session.states[0]));
        let before_rebase = session.snapshot();
        let absolute_x = f64::from(session.origin.x) + session.motion.position.x;
        let absolute_z = f64::from(session.origin.z) + session.motion.position.z;
        session.queue_chunk_window(neighbor, 0);
        session.update(TICK);
        let evicted = session.snapshot();
        let old_eye = before_rebase.camera.eye(1.0) + before_rebase.origin.as_dvec3();
        let new_eye = evicted.camera.eye(0.0) + evicted.origin.as_dvec3();
        assert!((old_eye - new_eye).length() < 1.0e-9);
        assert_eq!(
            f64::from(session.origin.x) + session.motion.position.x,
            absolute_x
        );
        assert_eq!(
            f64::from(session.origin.z) + session.motion.position.z,
            absolute_z
        );
        assert_eq!(loaded.origin, origin);
        assert!(!evicted.world.loaded().unwrap().chunks.contains_key(&home));
        assert!(matches!(
            session.saved_entity_chunk(home),
            Some(SavedEntityData::Empty)
        ));
        assert!(session.has_chunk_or_pending(home));
        assert_eq!(
            loaded
                .world
                .loaded()
                .unwrap()
                .block(origin.x + 8, 2, origin.z + 6)
                .unwrap()
                .name,
            "minecraft:stone"
        );
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        let restored = session.snapshot();
        assert_eq!(
            restored
                .world
                .loaded()
                .unwrap()
                .block(origin.x + 8, 2, origin.z + 6)
                .unwrap()
                .name,
            "minecraft:air"
        );
        assert!(
            !restored
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&neighbor)
        );
        assert!(session.saved_entity_chunk(neighbor).is_none());
        assert!(matches!(
            session.saved_entity_chunk(home),
            Some(SavedEntityData::Empty)
        ));
        assert!(!evicted.world.loaded().unwrap().chunks.contains_key(&home));
        assert!(session.take_stream_errors().is_empty());
    }

    #[test]
    fn stale_and_invalid_streamed_chunks_do_not_change_the_active_world() {
        let mut session = session();
        let home = ChunkPos {
            x: session.origin.x.div_euclid(16),
            z: session.origin.z.div_euclid(16),
        };
        let other = ChunkPos {
            x: home.x + 2,
            z: home.z,
        };
        session.queue_chunk_window(home, 1);
        session.queue_loaded_chunk(other, World::new("minecraft:overworld"));
        session.update(TICK);
        assert!(session.take_stream_errors().is_empty());
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&other)
        );
        let neighbor = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        session.queue_loaded_chunk(neighbor, World::new("minecraft:the_nether"));
        session.update(TICK);
        assert_eq!(session.take_stream_errors().len(), 1);
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&neighbor)
        );
    }

    #[test]
    fn ticks_and_shared_snapshots_do_not_depend_on_frame_cadence() {
        let mut a = session();
        let mut b = session();
        for session in [&mut a, &mut b] {
            session.submit_command(PlayerCommand::Move {
                forward: 1.0,
                strafe: 1.0,
                jump: false,
            });
            session.submit_command(PlayerCommand::Sprint {
                pressed: true,
                backward_pressed: false,
            });
            session.submit_command(PlayerCommand::Sneak(true));
        }
        let original = a.snapshot();
        for _ in 0..100 {
            a.update(Duration::from_millis(10));
        }
        for _ in 0..20 {
            b.update(TICK);
        }
        assert_eq!(a.snapshot().tick, 20);
        assert_eq!(a.snapshot().player.position, b.snapshot().player.position);
        assert_eq!(a.snapshot().sprinting, b.snapshot().sprinting);
        assert_eq!(a.snapshot().player.pose, b.snapshot().player.pose);
        assert!(Arc::ptr_eq(
            original.world.loaded().unwrap(),
            a.snapshot().world.loaded().unwrap()
        ));
        assert!(a.snapshot().player.on_ground);
    }

    #[test]
    fn loaded_world_hand_animation_does_not_depend_on_frame_cadence() {
        let mut a = session();
        let mut b = session();
        a.submit_command(PlayerCommand::Break);
        b.submit_command(PlayerCommand::Break);
        let mut saw_swing = false;
        for _ in 0..12 {
            for _ in 0..5 {
                a.update(Duration::from_millis(10));
            }
            b.update(TICK);
            assert_eq!(
                a.snapshot().first_person.swing,
                b.snapshot().first_person.swing
            );
            saw_swing |= a.snapshot().first_person.swing.progress(1.0) > 0.1;
            assert_eq!(
                a.snapshot().first_person.main.height,
                b.snapshot().first_person.main.height
            );
        }
        assert!(saw_swing);
        a.update(Duration::from_millis(25));
        b.update(Duration::from_millis(25));
        assert_eq!(a.partial_tick(), 0.5);
        assert_eq!(
            a.snapshot().first_person.swing.progress(a.partial_tick()),
            b.snapshot().first_person.swing.progress(b.partial_tick())
        );
    }

    #[test]
    fn sprint_input_is_applied_to_loaded_worlds_at_the_tick_boundary() {
        let mut session = session();
        let before = session.snapshot();
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: false,
        });
        session.submit_command(PlayerCommand::Sprint {
            pressed: true,
            backward_pressed: false,
        });
        session.update(Duration::from_millis(49));
        assert!(!session.snapshot().sprinting);
        session.update(Duration::from_millis(1));
        assert!(session.snapshot().sprinting);
        assert!(!before.sprinting);
    }

    #[test]
    fn crouching_fits_under_a_slab_and_standing_waits_for_clearance() {
        let (mut world, origin, mut player) = fixture();
        let slab = world.intern_state(state(
            "minecraft:oak_slab",
            &[("type", "top"), ("waterlogged", "false")],
        ));
        let pos = ChunkPos {
            x: origin.x.div_euclid(16),
            z: origin.z.div_euclid(16),
        };
        let chunk = Arc::make_mut(world.chunks.get_mut(&pos).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[8 + 8 * 16 + 2 * 256] = slab;
        player.position.x = 7.5;
        let mut session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session.update(TICK * 2);
        session.submit_command(PlayerCommand::Sneak(true));
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 1.0,
            jump: false,
        });
        for _ in 0..100 {
            session.update(TICK);
            if session.snapshot().player.position.x > 8.2 {
                break;
            }
        }
        assert!(session.snapshot().player.position.x > 8.2);
        assert_eq!(session.snapshot().player.pose, PlayerPose::Crouching);
        session.submit_command(PlayerCommand::Sneak(false));
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: false,
        });
        session.update(TICK);
        let player = session.snapshot().player;
        assert_eq!(player.pose, PlayerPose::Crouching);
        assert!((player.eye().y - (player.position.y + crate::CROUCH_EYE_HEIGHT)).abs() < 1.0e-6);
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: -1.0,
            jump: false,
        });
        for _ in 0..100 {
            session.update(TICK);
            if session.snapshot().player.pose == PlayerPose::Standing {
                break;
            }
        }
        assert_eq!(session.snapshot().player.pose, PlayerPose::Standing);
    }

    #[test]
    fn very_low_ceiling_selects_the_crawling_pose() {
        let (mut world, origin, player) = fixture();
        let slab = world.intern_state(state(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ));
        let pos = ChunkPos {
            x: origin.x.div_euclid(16),
            z: origin.z.div_euclid(16),
        };
        let chunk = Arc::make_mut(world.chunks.get_mut(&pos).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[8 + 8 * 16 + 2 * 256] = slab;
        let mut session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session.update(TICK);
        let player = session.snapshot().player;
        assert_eq!(player.pose, PlayerPose::Crawling);
        assert!((player.eye().y - (player.position.y + crate::CRAWL_EYE_HEIGHT)).abs() < 1.0e-6);
    }

    #[test]
    fn interactions_change_only_new_snapshots_and_keep_origin_precision() {
        let mut session = session();
        session.set_game_mode(GameMode::Creative);
        let before = session.snapshot();
        let target = session.origin + IVec3::new(8, 2, 6);
        assert_eq!(
            before
                .world
                .loaded()
                .unwrap()
                .block(target.x, target.y, target.z)
                .unwrap()
                .name,
            "minecraft:stone"
        );
        session.submit_command(PlayerCommand::Break);
        session.update(TICK);
        let after = session.snapshot();
        assert_eq!(
            before
                .world
                .loaded()
                .unwrap()
                .block(target.x, target.y, target.z)
                .unwrap()
                .name,
            "minecraft:stone"
        );
        assert_eq!(
            after
                .world
                .loaded()
                .unwrap()
                .block(target.x, target.y, target.z)
                .unwrap()
                .name,
            "minecraft:air"
        );
        assert!(!Arc::ptr_eq(
            before.world.loaded().unwrap(),
            after.world.loaded().unwrap()
        ));
        assert_eq!(after.world_revision, 1);

        let mut session = self::session();
        session.submit_command(PlayerCommand::Select(Block::Dirt));
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        let adjacent = session.origin + IVec3::new(8, 2, 7);
        assert_eq!(
            session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .block(adjacent.x, adjacent.y, adjacent.z)
                .unwrap()
                .name,
            "minecraft:dirt"
        );
    }

    #[test]
    fn placing_a_saved_stack_respects_mode_and_preserves_old_snapshots() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        for mode in [
            GameMode::Survival,
            GameMode::Creative,
            GameMode::Adventure,
            GameMode::Spectator,
        ] {
            let mut session = session();
            session.set_game_mode(mode);
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(
                    "minecraft:stone_bricks".into(),
                    1,
                    64,
                    Tag::Compound(vec![]),
                ),
            );
            session.set_player_inventory(inventory);
            let before = session.snapshot();
            let target = session.origin + IVec3::new(8, 2, 7);
            session.submit_command(PlayerCommand::Place);
            session.update(TICK);
            let after = session.snapshot();
            let placed = matches!(mode, GameMode::Survival | GameMode::Creative);
            assert_eq!(
                after
                    .world
                    .loaded()
                    .unwrap()
                    .block(target.x, target.y, target.z)
                    .unwrap()
                    .name,
                if placed {
                    "minecraft:stone_bricks"
                } else {
                    "minecraft:air"
                }
            );
            assert_eq!(
                after.inventory.slot(PlayerSlot::Storage(0)).is_none(),
                mode == GameMode::Survival
            );
            assert_eq!(
                before
                    .inventory
                    .slot(PlayerSlot::Storage(0))
                    .unwrap()
                    .count(),
                1
            );
            assert_eq!(
                before
                    .world
                    .loaded()
                    .unwrap()
                    .block(target.x, target.y, target.z)
                    .unwrap()
                    .name,
                "minecraft:air"
            );
        }
    }

    #[test]
    fn loaded_world_places_from_offhand_when_main_hand_is_empty() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        for mode in [GameMode::Survival, GameMode::Creative] {
            let mut session = session();
            session.set_game_mode(mode);
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Offhand,
                ItemStack::new(
                    "minecraft:stone_bricks".into(),
                    2,
                    64,
                    Tag::Compound(vec![]),
                ),
            );
            session.set_player_inventory(inventory);
            let target = session.origin + IVec3::new(8, 2, 7);
            session.submit_command(PlayerCommand::Place);
            session.update(TICK);
            let after = session.snapshot();
            assert_eq!(
                after.first_person.swing.hand(),
                Some(crate::animation::Hand::Off)
            );
            assert_eq!(
                after
                    .world
                    .loaded()
                    .unwrap()
                    .block(target.x, target.y, target.z)
                    .unwrap()
                    .name,
                "minecraft:stone_bricks"
            );
            assert_eq!(
                after.inventory.slot(PlayerSlot::Offhand).unwrap().count(),
                if mode == GameMode::Survival { 1 } else { 2 }
            );
            assert!(after.inventory.slot(PlayerSlot::Storage(0)).is_none());
        }
    }

    #[test]
    fn loaded_crafting_table_opens_at_absolute_position_and_closes_when_removed() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        let mut session = session();
        let relative = IVec3::new(8, 2, 6);
        let id = session.intern_block(state("minecraft:crafting_table", &[]));
        assert!(session.set(relative, id));
        let original = session.snapshot();
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        assert_eq!(
            session.snapshot().inventory.menu(),
            Some(InventoryMenu::CraftingTable {
                position: session.origin + relative
            })
        );
        Arc::make_mut(&mut session.inventory).set_slot(
            PlayerSlot::Crafting(8),
            ItemStack::new("minecraft:dirt".into(), 2, 64, Tag::Compound(vec![])),
        );
        assert!(session.set(relative, session.states[0]));
        session.update(TICK);
        let after = session.snapshot();
        assert!(!after.inventory.is_open());
        assert!(
            after
                .inventory
                .slot(PlayerSlot::Storage(3))
                .is_some_and(|item| item.id() == "minecraft:dirt" && item.count() == 2)
        );
        assert!(!original.inventory.is_open());
    }

    #[test]
    fn loaded_world_places_supported_blocks_from_either_hand() {
        use crate::inventory::{ItemStack, PlayerSlot};
        use native_core::wood::Planks;
        use native_persistence::nbt::Tag;
        for block in Planks::ALL
            .into_iter()
            .map(Block::Planks)
            .chain([
                Block::CraftingTable,
                Block::Cobblestone,
                Block::Sand,
                Block::RedSand,
                Block::Gravel,
            ])
            .chain(
                native_core::wood::Wood::all().map(|wood| Block::Wood(wood, native_world::Axis::Y)),
            )
        {
            for hand in [PlayerSlot::Storage(0), PlayerSlot::Offhand] {
                for mode in [GameMode::Survival, GameMode::Creative, GameMode::Adventure] {
                    let mut session = session();
                    session.set_game_mode(mode);
                    let mut inventory = PlayerInventory::default();
                    inventory.set_slot(
                        hand,
                        ItemStack::new(
                            block.item_id().unwrap().into(),
                            2,
                            64,
                            Tag::Compound(vec![]),
                        ),
                    );
                    session.set_player_inventory(inventory);
                    let old = session.snapshot();
                    let target = session.origin + IVec3::new(8, 2, 7);
                    session.submit_command(PlayerCommand::Place);
                    session.update(TICK);
                    let after = session.snapshot();
                    assert_eq!(
                        old.world
                            .loaded()
                            .unwrap()
                            .block(target.x, target.y, target.z)
                            .unwrap()
                            .name,
                        "minecraft:air"
                    );
                    let placed = mode != GameMode::Adventure;
                    let state = after
                        .world
                        .loaded()
                        .unwrap()
                        .block(target.x, target.y, target.z)
                        .unwrap();
                    assert_eq!(
                        state.name,
                        if placed {
                            block.item_id().unwrap()
                        } else {
                            "minecraft:air"
                        }
                    );
                    if placed {
                        if matches!(block, Block::Wood(_, _)) {
                            assert_eq!(state.properties.get("axis").map(String::as_str), Some("z"));
                        } else {
                            assert!(state.properties.is_empty());
                        }
                        assert!(block_shape(state).is_some());
                        assert!(block_outline(state).is_some());
                    }
                    assert_eq!(
                        after.inventory.slot(hand).unwrap().count(),
                        if mode == GameMode::Survival { 1 } else { 2 }
                    );
                }
            }
        }
    }

    #[test]
    fn stone_button_places_on_known_full_support_from_either_hand() {
        use crate::inventory::PlayerSlot;
        use native_persistence::nbt::Tag;
        for hand in [PlayerSlot::Storage(0), PlayerSlot::Offhand] {
            for mode in [GameMode::Survival, GameMode::Creative, GameMode::Adventure] {
                let mut session = session();
                session.set_game_mode(mode);
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    hand,
                    ItemStack::new(
                        "minecraft:stone_button".into(),
                        2,
                        64,
                        Tag::Compound(vec![]),
                    ),
                );
                session.set_player_inventory(inventory);
                session.submit_command(PlayerCommand::Place);
                session.update(TICK);
                let target = session.origin + IVec3::new(8, 2, 7);
                let placed = session.world.block(target.x, target.y, target.z).unwrap();
                if mode == GameMode::Adventure {
                    assert_eq!(placed.name, "minecraft:air");
                } else {
                    assert_eq!(placed, &stone_button_state(IVec3::Z, 0.0).unwrap());
                }
                assert_eq!(
                    session.inventory.slot(hand).unwrap().count(),
                    if mode == GameMode::Survival { 1 } else { 2 }
                );
            }
        }
    }

    #[test]
    fn stone_button_places_on_each_face_and_requires_free_target() {
        use crate::inventory::PlayerSlot;
        use native_persistence::nbt::Tag;
        for face in [
            IVec3::X,
            IVec3::NEG_X,
            IVec3::Y,
            IVec3::NEG_Y,
            IVec3::Z,
            IVec3::NEG_Z,
        ] {
            let mut session = session();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(
                    "minecraft:stone_button".into(),
                    1,
                    64,
                    Tag::Compound(vec![]),
                ),
            );
            session.set_player_inventory(inventory);
            let hit = ShapeRayHit {
                position: IVec3::new(8, 2, 6),
                face,
                location: DVec3::ZERO,
                inside: false,
            };
            assert_eq!(
                session.place_stone_button(hit),
                Some(PlayerSlot::Storage(0))
            );
            let target = session.origin + hit.position + face;
            assert_eq!(
                session.world.block(target.x, target.y, target.z),
                Some(&stone_button_state(face, session.player.yaw).unwrap())
            );
            assert!(session.place_stone_button(hit).is_none());
        }
    }

    #[test]
    fn slab_support_controls_button_placement_and_neighbor_updates() {
        use crate::inventory::PlayerSlot;
        use native_persistence::nbt::Tag;
        for kind in ["bottom", "top", "double"] {
            for face in [
                IVec3::X,
                IVec3::NEG_X,
                IVec3::Y,
                IVec3::NEG_Y,
                IVec3::Z,
                IVec3::NEG_Z,
            ] {
                let sturdy = kind == "double"
                    || (kind == "top" && face == IVec3::Y)
                    || (kind == "bottom" && face == IVec3::NEG_Y);
                let mut placement = session();
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    PlayerSlot::Storage(0),
                    ItemStack::new(
                        "minecraft:stone_button".into(),
                        1,
                        64,
                        Tag::Compound(vec![]),
                    ),
                );
                placement.set_player_inventory(inventory);
                let support = IVec3::new(8, 2, 6);
                let slab = placement.intern_block(state(
                    "minecraft:oak_slab",
                    &[("type", kind), ("waterlogged", "false")],
                ));
                assert!(placement.set(support, slab));
                if face == IVec3::NEG_Y {
                    assert!(placement.set(support + face + IVec3::NEG_Y, placement.states[0]));
                }
                let hit = ShapeRayHit {
                    position: support,
                    face,
                    location: DVec3::ZERO,
                    inside: false,
                };
                assert_eq!(
                    placement.place_stone_button(hit).is_some(),
                    sturdy,
                    "{kind} {face:?}"
                );

                let mut changed = session();
                let button_pos = support + face;
                let button_id = changed.intern_block(stone_button_state(face, 0.0).unwrap());
                assert!(changed.set(button_pos, button_id));
                let slab_id = changed.intern_block(state(
                    "minecraft:oak_slab",
                    &[("type", kind), ("waterlogged", "false")],
                ));
                assert!(changed.set(support, slab_id));
                let Cell::State(after) = cell(
                    &changed.world,
                    changed.origin,
                    changed.bounds,
                    button_pos,
                    changed.states[0],
                ) else {
                    panic!("unloaded target")
                };
                assert_eq!(
                    after == button_id,
                    sturdy,
                    "changed support {kind} {face:?}"
                );
            }
        }
    }

    #[test]
    fn straight_stair_supports_buttons_only_on_its_full_faces() {
        use crate::inventory::PlayerSlot;
        use native_persistence::nbt::Tag;
        for face in [
            IVec3::X,
            IVec3::NEG_X,
            IVec3::Y,
            IVec3::NEG_Y,
            IVec3::Z,
            IVec3::NEG_Z,
        ] {
            let mut session = session();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(
                    "minecraft:stone_button".into(),
                    1,
                    64,
                    Tag::Compound(vec![]),
                ),
            );
            session.set_player_inventory(inventory);
            let support = IVec3::new(8, 2, 6);
            let stairs = session.intern_block(state(
                "minecraft:oak_stairs",
                &[
                    ("facing", "north"),
                    ("half", "bottom"),
                    ("shape", "straight"),
                    ("waterlogged", "false"),
                ],
            ));
            assert!(session.set(support, stairs));
            if face == IVec3::NEG_Y {
                assert!(session.set(support + face + IVec3::NEG_Y, session.states[0]));
            }
            let hit = ShapeRayHit {
                position: support,
                face,
                location: DVec3::ZERO,
                inside: false,
            };
            assert_eq!(
                session.place_stone_button(hit).is_some(),
                matches!(face, IVec3::NEG_Y | IVec3::NEG_Z),
                "{face:?}"
            );
        }
    }

    #[test]
    fn button_fallback_respects_candidate_order_and_unknown_support() {
        use crate::inventory::PlayerSlot;
        use native_persistence::nbt::Tag;
        let mut session = session();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new(
                "minecraft:stone_button".into(),
                1,
                64,
                Tag::Compound(vec![]),
            ),
        );
        session.set_player_inventory(inventory);
        let support = IVec3::new(8, 2, 6);
        let slab = session.intern_block(state(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ));
        assert!(session.set(support, slab));
        let hit = ShapeRayHit {
            position: support,
            face: IVec3::Y,
            location: DVec3::ZERO,
            inside: false,
        };
        let target = support + hit.face;
        let directions =
            button_placement_directions(session.player.yaw, session.player.pitch, hit.face, false)
                .unwrap();
        let unknown = session.intern_block(state("minecraft:unknown_support", &[]));
        assert!(session.set(target + directions[1], unknown));
        assert!(session.set(target + directions[2], session.states[3]));
        assert!(session.place_stone_button(hit).is_none());
        assert!(session.set(target + directions[1], session.states[0]));
        assert_eq!(
            session.place_stone_button(hit),
            Some(PlayerSlot::Storage(0))
        );
        let Cell::State(id) = cell(
            &session.world,
            session.origin,
            session.bounds,
            target,
            session.states[0],
        ) else {
            panic!("missing target")
        };
        assert_eq!(
            session.world.state(id),
            Some(&stone_button_state(-directions[2], session.player.yaw).unwrap())
        );
    }

    #[test]
    fn button_replaces_clicked_or_adjacent_supported_plants() {
        use crate::inventory::PlayerSlot;
        use native_persistence::nbt::Tag;
        for plant in [
            "minecraft:short_grass",
            "minecraft:fern",
            "minecraft:dead_bush",
            "minecraft:fire",
        ] {
            for clicked in [false, true] {
                let mut session = session();
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    PlayerSlot::Storage(0),
                    ItemStack::new(
                        "minecraft:stone_button".into(),
                        2,
                        64,
                        Tag::Compound(vec![]),
                    ),
                );
                session.set_player_inventory(inventory);
                let support = IVec3::new(8, 2, 6);
                let target = support + IVec3::Z;
                let plant_id = session.intern_block(state(plant, &[]));
                assert!(session.set(target, plant_id));
                let hit = ShapeRayHit {
                    position: if clicked { target } else { support },
                    face: IVec3::Z,
                    location: DVec3::ZERO,
                    inside: false,
                };
                assert_eq!(
                    session.place_stone_button(hit),
                    Some(PlayerSlot::Storage(0)),
                    "{plant}, clicked={clicked}"
                );
                let Cell::State(result) = cell(
                    &session.world,
                    session.origin,
                    session.bounds,
                    target,
                    session.states[0],
                ) else {
                    panic!("missing target")
                };
                assert_eq!(
                    session.world.state(result).unwrap().name,
                    "minecraft:stone_button"
                );
                assert_eq!(
                    stone_button_support_direction(session.world.state(result).unwrap()),
                    Some(IVec3::NEG_Z)
                );
            }
        }
    }

    #[test]
    fn clicking_grass_places_button_in_its_cell() {
        use crate::inventory::PlayerSlot;
        use native_persistence::nbt::Tag;
        let mut session = session();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new(
                "minecraft:stone_button".into(),
                2,
                64,
                Tag::Compound(vec![]),
            ),
        );
        session.set_player_inventory(inventory);
        let target = IVec3::new(8, 2, 7);
        let grass = session.intern_block(state("minecraft:short_grass", &[]));
        assert!(session.set(target, grass));
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        let absolute = session.origin + target;
        assert_eq!(
            session
                .world
                .block(absolute.x, absolute.y, absolute.z)
                .unwrap()
                .name,
            "minecraft:stone_button"
        );
        assert_eq!(
            session
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn button_can_replace_adjacent_ordinary_fluid() {
        use crate::inventory::PlayerSlot;
        use native_persistence::nbt::Tag;
        for fluid in ["minecraft:water", "minecraft:lava"] {
            let mut session = session();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(
                    "minecraft:stone_button".into(),
                    1,
                    64,
                    Tag::Compound(vec![]),
                ),
            );
            session.set_player_inventory(inventory);
            let support = IVec3::new(8, 2, 6);
            let target = support + IVec3::Z;
            let id = session.intern_block(state(fluid, &[("level", "0")]));
            assert!(session.set(target, id));
            let hit = ShapeRayHit {
                position: support,
                face: IVec3::Z,
                location: DVec3::ZERO,
                inside: false,
            };
            assert_eq!(
                session.place_stone_button(hit),
                Some(PlayerSlot::Storage(0)),
                "{fluid}"
            );
            let Cell::State(result) = cell(
                &session.world,
                session.origin,
                session.bounds,
                target,
                session.states[0],
            ) else {
                panic!("missing target")
            };
            assert_eq!(
                session.world.state(result).unwrap().name,
                "minecraft:stone_button"
            );
        }
    }

    #[test]
    fn removing_button_support_updates_six_faces_and_uses_world_loot() {
        use native_resources::{loot::BlockLootTables, packs::ResourceProvider};
        struct LootSource;
        impl ResourceProvider for LootSource {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(path.ends_with("/stone_button.json").then(|| br#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:diamond"}]}]}"#.to_vec()))
            }
        }
        let loot = Arc::new(BlockLootTables::load(&mut LootSource).unwrap());
        for face in [
            IVec3::X,
            IVec3::NEG_X,
            IVec3::Y,
            IVec3::NEG_Y,
            IVec3::Z,
            IVec3::NEG_Z,
        ] {
            for mode in [GameMode::Survival, GameMode::Creative] {
                for drops in [false, true] {
                    for powered in [false, true] {
                        let mut session = session();
                        session.set_block_loot(Arc::clone(&loot));
                        session.set_block_drops(drops);
                        session.set_game_mode(mode);
                        let support = IVec3::new(8, 2, 6);
                        let button = support + face;
                        let mut button_state = stone_button_state(face, 0.0).unwrap();
                        button_state
                            .properties
                            .insert("powered".into(), powered.to_string());
                        let id = session.intern_block(button_state);
                        assert!(session.set(button, id));
                        let unrelated = button + if face.x == 0 { IVec3::X } else { IVec3::Y };
                        assert!(session.set(unrelated, session.states[3]));
                        assert!(session.set(unrelated, session.states[0]));
                        assert!(
                            matches!(cell(&session.world, session.origin, session.bounds, button, session.states[0]), Cell::State(current) if current == id)
                        );
                        assert!(session.set(support, session.states[0]));
                        assert!(
                            matches!(cell(&session.world, session.origin, session.bounds, button, session.states[0]), Cell::State(current) if current == session.states[0])
                        );
                        let items = session.dropped_items.snapshot();
                        assert_eq!(
                            items.len(),
                            usize::from(drops),
                            "{face:?} {mode:?} powered={powered}"
                        );
                        if drops {
                            assert_eq!(items[0].stack.id(), "minecraft:diamond");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn stone_button_press_releases_after_twenty_ticks_without_extending() {
        for mode in [
            GameMode::Survival,
            GameMode::Creative,
            GameMode::Adventure,
            GameMode::Spectator,
        ] {
            let mut session = session();
            session.set_game_mode(mode);
            let position = IVec3::new(8, 2, 7);
            let id = session.intern_block(stone_button_state(IVec3::Z, 0.0).unwrap());
            assert!(session.set(position, id));
            session.submit_command(PlayerCommand::Place);
            session.update(TICK);
            let absolute = session.origin + position;
            let powered = |s: &LoadedSession| {
                s.world
                    .block(absolute.x, absolute.y, absolute.z)
                    .unwrap()
                    .properties["powered"]
                    .clone()
            };
            if mode == GameMode::Spectator {
                assert_eq!(powered(&session), "false");
                assert!(session.button_ticks.is_empty());
                assert!(session.take_button_sounds().is_empty());
                continue;
            }
            assert_eq!(powered(&session), "true");
            assert_eq!(
                session.take_button_sounds(),
                vec![ButtonSoundEvent {
                    position: absolute,
                    powered: true,
                    tick: 0
                }]
            );
            for _ in 0..18 {
                session.update(TICK);
            }
            session.submit_command(PlayerCommand::Place);
            session.update(TICK);
            assert_eq!(powered(&session), "true");
            assert_eq!(session.button_ticks.len(), 1);
            assert!(session.take_button_sounds().is_empty());
            session.update(TICK);
            assert_eq!(powered(&session), "false");
            assert!(session.button_ticks.is_empty());
            assert_eq!(
                session.take_button_sounds(),
                vec![ButtonSoundEvent {
                    position: absolute,
                    powered: false,
                    tick: 20
                }]
            );
            assert!(session.take_button_sounds().is_empty());
        }
    }

    #[test]
    fn button_sound_events_are_independent_of_frame_spacing_and_snapshots() {
        fn run(steps: &[Duration]) -> Vec<ButtonSoundEvent> {
            let mut session = session();
            let position = IVec3::new(8, 2, 7);
            let id = session.intern_block(stone_button_state(IVec3::Z, 0.0).unwrap());
            assert!(session.set(position, id));
            session.submit_command(PlayerCommand::Place);
            let mut events = Vec::new();
            for step in steps {
                session.update(*step);
                let _ = session.snapshot();
                let _ = session.snapshot();
                events.extend(session.take_button_sounds());
                assert!(session.take_button_sounds().is_empty());
            }
            events
        }
        let steady = run(&vec![Duration::from_millis(10); 110]);
        let irregular = run(&[
            Duration::from_millis(250),
            Duration::from_millis(100),
            Duration::from_millis(250),
            Duration::from_millis(250),
            Duration::from_millis(250),
        ]);
        assert_eq!(steady, irregular);
        assert_eq!(steady.len(), 2);
        assert_eq!((steady[0].tick, steady[1].tick), (0, 20));
    }

    #[test]
    fn stone_button_mining_uses_tools_and_data_pack_loot() {
        use crate::inventory::PlayerSlot;
        use native_persistence::nbt::Tag;
        use native_resources::{loot::BlockLootTables, packs::ResourceProvider};
        struct LootSource;
        impl ResourceProvider for LootSource {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(path.ends_with("/stone_button.json").then(|| br#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone_button"}]}]}"#.to_vec()))
            }
        }
        let loot = Arc::new(BlockLootTables::load(&mut LootSource).unwrap());
        let mut durations = Vec::new();
        for (tool, mode) in [
            (None, GameMode::Survival),
            (Some("minecraft:stone_pickaxe"), GameMode::Survival),
            (None, GameMode::Creative),
        ] {
            let mut session = session();
            session.set_game_mode(mode);
            session.set_block_loot(Arc::clone(&loot));
            let mut inventory = PlayerInventory::default();
            if let Some(tool) = tool {
                inventory.set_slot(
                    PlayerSlot::Storage(0),
                    ItemStack::new(tool.into(), 1, 1, Tag::Compound(vec![])),
                );
            }
            session.set_player_inventory(inventory);
            let relative = IVec3::new(8, 2, 7);
            let id = session.intern_block(stone_button_state(IVec3::Z, 0.0).unwrap());
            assert!(session.set(relative, id));
            session.update(TICK * 3);
            session.submit_command(PlayerCommand::Break);
            let mut ticks = 0;
            loop {
                session.update(TICK);
                ticks += 1;
                let current = cell(
                    &session.world,
                    session.origin,
                    session.bounds,
                    relative,
                    session.states[0],
                );
                if matches!(current, Cell::State(block) if block == session.states[0]) {
                    break;
                }
                assert!(ticks < 100, "button did not break: {tool:?} {mode:?}");
            }
            durations.push(ticks);
            let items = session.dropped_items.snapshot();
            assert_eq!(items.len(), usize::from(mode != GameMode::Creative));
            if mode != GameMode::Creative {
                assert_eq!(items[0].stack.id(), "minecraft:stone_button");
            }
            if tool.is_some() {
                assert_eq!(
                    session
                        .inventory
                        .slot(PlayerSlot::Storage(0))
                        .unwrap()
                        .components()
                        .get("minecraft:damage"),
                    Some(&Tag::Int(1))
                );
            }
        }
        assert!(durations[1] < durations[0]);
        assert_eq!(durations[2], 1);
    }

    #[test]
    fn saved_button_ticks_wait_for_loaded_chunks_and_are_included_in_drafts() {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let remote = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let position = IVec3::new(8, 2, 7);
        let absolute = session.origin + position;
        let mut state = stone_button_state(IVec3::Z, 0.0).unwrap();
        state.properties.insert("powered".into(), "true".into());
        let id = session.intern_block(state);
        assert!(session.set(position, id));
        session.enqueue_saved_buttons(&[SavedBlockTick {
            block: "minecraft:stone_button".into(),
            position: absolute.to_array(),
            delay: 2,
            priority: 0,
        }]);
        assert_eq!(
            session.draft_block_ticks(home).unwrap(),
            vec![SavedBlockTick {
                block: "minecraft:stone_button".into(),
                position: absolute.to_array(),
                delay: 2,
                priority: 0,
            }]
        );
        session.queue_chunk_window(remote, 0);
        for _ in 0..5 {
            session.update(TICK);
        }
        assert_eq!(session.button_deferred.len(), 1);
        assert_eq!(session.draft_block_ticks(home).unwrap()[0].delay, -3);
        session.queue_chunk_window(home, 0);
        session.update(TICK);
        assert_eq!(
            session
                .world
                .block(absolute.x, absolute.y, absolute.z)
                .unwrap()
                .properties["powered"],
            "false"
        );
        assert!(session.button_ticks.is_empty() && session.button_deferred.is_empty());
        assert!(session.draft_block_ticks(home).unwrap().is_empty());
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn button_placement_state_matches_java_on_stone() {
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
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ButtonPlacementOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/ButtonPlacementOracle.java"),
        )
        .unwrap();
        let faces = [
            ("UP", IVec3::Y),
            ("DOWN", IVec3::NEG_Y),
            ("NORTH", IVec3::NEG_Z),
            ("SOUTH", IVec3::Z),
            ("WEST", IVec3::NEG_X),
            ("EAST", IVec3::X),
        ];
        let mut inputs = Vec::new();
        let mut expected = Vec::new();
        for (face_name, face) in faces {
            for degrees in [
                -180.0_f32, -135.01, -135.0, -90.0, -45.01, -45.0, 0.0, 44.99, 45.0, 90.0, 135.0,
                180.0, 270.0,
            ] {
                let yaw = degrees.to_radians();
                inputs.push(serde_json::json!({"face":face_name,"yaw":yaw.to_degrees()-180.0}));
                let button = stone_button_state(face, yaw).unwrap();
                assert_eq!(stone_button_support_direction(&button), Some(-face));
                expected.push(button.properties);
            }
        }
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&inputs).unwrap()).unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java button oracle failed:\n{}\n{}",
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
            .arg("ButtonPlacementOracle")
            .arg(&input)
            .arg(&output));
        let actual: Vec<BTreeMap<String, String>> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(actual, expected);

        let source = temp.path().join("ButtonFallbackOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/ButtonFallbackOracle.java"),
        )
        .unwrap();
        let directions = [
            ("DOWN", IVec3::NEG_Y),
            ("UP", IVec3::Y),
            ("NORTH", IVec3::NEG_Z),
            ("SOUTH", IVec3::Z),
            ("WEST", IVec3::NEG_X),
            ("EAST", IVec3::X),
        ];
        let mut inputs = Vec::new();
        let mut expected = Vec::new();
        for (face_name, face) in faces {
            for yaw_degrees in [
                -180.0_f32, -135.01, -135.0, -90.0, -45.01, -45.0, 0.0, 44.99, 45.0, 90.0, 135.0,
                180.0, 270.0,
            ] {
                for pitch_degrees in [-89.0_f32, -45.0, -0.01, 0.0, 0.01, 45.0, 89.0] {
                    let yaw = yaw_degrees.to_radians();
                    let pitch = pitch_degrees.to_radians();
                    for replace_clicked in [false, true] {
                        let order =
                            button_placement_directions(yaw, pitch, face, replace_clicked).unwrap();
                        let order_names: Vec<_> = order
                            .iter()
                            .map(|direction| {
                                directions
                                    .iter()
                                    .find(|(_, value)| value == direction)
                                    .unwrap()
                                    .0
                            })
                            .collect();
                        for mask in [0, 1, 2, 4, 8, 16, 32, 21, 42, 63] {
                            let first = order.iter().find(|direction| {
                                let index = directions
                                    .iter()
                                    .position(|(_, value)| value == *direction)
                                    .unwrap();
                                mask & (1 << index) != 0
                            });
                            let properties = first.map(|direction| {
                                stone_button_state(-*direction, yaw).unwrap().properties
                            });
                            inputs.push(serde_json::json!({"face":face_name,"yaw":yaw.to_degrees()-180.0,"pitch":-pitch.to_degrees(),"mask":mask,"replace":replace_clicked}));
                            expected
                                .push(serde_json::json!({"order":order_names,"state":properties}));
                        }
                    }
                }
            }
        }
        let input = temp.path().join("fallback-input.json");
        let output = temp.path().join("fallback-output.json");
        std::fs::write(&input, serde_json::to_vec(&inputs).unwrap()).unwrap();
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("ButtonFallbackOracle")
            .arg(&input)
            .arg(&output));
        let actual: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_eq!(actual, expected, "fallback case {index}: {}", inputs[index]);
        }
    }

    #[test]
    fn restricted_modes_do_not_break_blocks_with_basic_items() {
        for mode in [GameMode::Adventure, GameMode::Spectator] {
            let mut session = session();
            session.set_game_mode(mode);
            let target = session.origin + IVec3::new(8, 2, 6);
            session.submit_command(PlayerCommand::Break);
            session.update(TICK);
            assert_eq!(
                session
                    .snapshot()
                    .world
                    .loaded()
                    .unwrap()
                    .block(target.x, target.y, target.z)
                    .unwrap()
                    .name,
                "minecraft:stone"
            );
        }
    }

    #[test]
    fn breaking_progress_uses_absolute_coordinates_and_shared_snapshots() {
        let mut session = session();
        session.update(TICK * 3);
        let target = session.origin + IVec3::new(8, 2, 6);
        session.submit_command(PlayerCommand::Break);
        session.update(TICK * 3);
        let old = session.snapshot();
        assert_eq!(old.breaking.unwrap().position, target);
        assert_eq!(
            old.world
                .loaded()
                .unwrap()
                .block(target.x, target.y, target.z)
                .unwrap()
                .name,
            "minecraft:stone"
        );
        session.submit_command(PlayerCommand::StopBreak);
        session.update(TICK);
        assert!(session.snapshot().breaking.is_none());
        assert!(old.breaking.is_some());
    }

    #[test]
    fn targeting_passes_through_the_empty_half_of_a_slab() {
        let (mut world, origin, player) = fixture();
        let slab = world.intern_state(state(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ));
        world
            .set_block_id(origin.x + 8, 2, origin.z + 7, slab)
            .unwrap();
        let mut session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session.set_game_mode(GameMode::Creative);
        session.submit_command(PlayerCommand::Break);
        session.update(TICK);
        assert_eq!(
            session
                .world
                .block(origin.x + 8, 2, origin.z + 7)
                .unwrap()
                .name,
            "minecraft:oak_slab"
        );
        assert_eq!(
            session
                .world
                .block(origin.x + 8, 2, origin.z + 6)
                .unwrap()
                .name,
            "minecraft:air"
        );
    }

    #[test]
    fn targeting_button_hits_its_outline_and_passes_through_empty_space() {
        let (mut world, origin, mut player) = fixture();
        let button = world.intern_state(state(
            "minecraft:stone_button",
            &[("face", "wall"), ("facing", "north"), ("powered", "false")],
        ));
        world
            .set_block_id(origin.x + 8, 2, origin.z + 7, button)
            .unwrap();
        let make = |player| {
            LoadedSession::new(
                world.clone(),
                origin,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap()
        };
        let hit = make(player).ray_hit().unwrap();
        assert_eq!(hit.position, IVec3::new(8, 2, 7));
        player.position.x = 8.8;
        let hit = make(player).ray_hit().unwrap();
        assert_eq!(hit.position, IVec3::new(8, 2, 6));
    }

    #[test]
    fn missing_chunks_are_barriers_but_omitted_full_chunk_sections_are_air() {
        let mut session = session();
        assert!(matches!(
            cell(
                &session.world,
                session.origin,
                session.bounds,
                IVec3::new(16, 8, 8),
                session.states[0]
            ),
            Cell::Missing
        ));
        assert!(
            matches!(cell(&session.world, session.origin, session.bounds, IVec3::new(8, 16, 8), session.states[0]), Cell::State(id) if id == session.states[0])
        );
        let original = session.snapshot();
        assert!(!session.set(IVec3::new(8, 16, 8), session.states[0]));
        assert!(!session.set(IVec3::new(16, 1, 8), session.states[3]));
        assert!(Arc::ptr_eq(
            original.world.loaded().unwrap(),
            &session.world
        ));
        let chunk_pos = ChunkPos {
            x: session.origin.x.div_euclid(16),
            z: session.origin.z.div_euclid(16),
        };
        let world = Arc::make_mut(&mut session.world);
        Arc::make_mut(world.chunks.get_mut(&chunk_pos).unwrap())
            .sections
            .insert(
                1,
                Arc::new(Section {
                    blocks: None,
                    biomes: None,
                    block_light: Some(Box::new([0x44; LIGHT_BYTES])),
                    sky_light: None,
                }),
            );
        assert!(session.set(IVec3::new(8, 16, 8), session.states[3]));
        let section = &session.world.chunks[&chunk_pos].sections[&1];
        assert_eq!(section.block_light.as_ref().unwrap()[0], 0x44);
        assert!(section.blocks.is_some());
        assert!(
            !original.world.loaded().unwrap().chunks[&chunk_pos]
                .sections
                .contains_key(&1)
        );
        assert!(!session.set(IVec3::new(16, 1, 8), session.states[3]));
        assert!(!session.set(IVec3::new(8, 32, 8), session.states[3]));
    }

    #[test]
    fn slab_collision_uses_its_half_height() {
        let (mut world, origin, player) = fixture();
        let slab = world.intern_state(state(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ));
        world
            .set_block_id(origin.x + 8, 0, origin.z + 8, slab)
            .unwrap();
        let session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let mut pos = Vec3::new(8.5, 3.0, 8.5);
        assert!(move_axis(
            &session.world,
            &session.shapes,
            origin,
            session.bounds,
            session.states[0],
            &mut pos,
            1,
            -4.0
        ));
        assert_eq!(pos.y, 0.5);
    }

    #[test]
    fn player_walks_up_a_slab_but_not_a_full_block_or_under_a_low_ceiling() {
        let make = |slab: bool, ceiling: bool| {
            let (mut world, origin, player) = fixture();
            let obstacle = if slab {
                world.intern_state(state(
                    "minecraft:oak_slab",
                    &[("type", "bottom"), ("waterlogged", "false")],
                ))
            } else {
                world.intern_state(state("minecraft:stone", &[]))
            };
            world
                .set_block_id(origin.x + 9, 1, origin.z + 8, obstacle)
                .unwrap();
            if ceiling {
                let stone = world.intern_state(state("minecraft:stone", &[]));
                for x in [8, 9] {
                    world
                        .set_block_id(origin.x + x, 3, origin.z + 8, stone)
                        .unwrap();
                }
            }
            LoadedSession::new(
                world,
                origin,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap()
        };
        let mut open = make(true, false);
        let mut wall = make(false, false);
        let mut low = make(true, true);
        for session in [&mut open, &mut wall, &mut low] {
            session.submit_command(PlayerCommand::Move {
                forward: 0.0,
                strafe: 1.0,
                jump: false,
            });
            for _ in 0..7 {
                session.update(TICK);
            }
        }
        assert!(open.player.position.x > 9.0);
        assert!((open.player.position.y - 1.5).abs() < 1.0e-5);
        for blocked in [wall, low] {
            assert!(blocked.player.position.x <= 8.700_01);
            assert!((blocked.player.position.y - 1.0).abs() < 1.0e-5);
        }
    }

    #[test]
    fn ice_under_the_player_preserves_more_horizontal_velocity() {
        let make = |ice: bool| {
            let (mut world, origin, mut player) = fixture();
            if ice {
                let block = world.intern_state(state("minecraft:ice", &[]));
                world
                    .set_block_id(origin.x + 8, 0, origin.z + 8, block)
                    .unwrap();
            }
            player.on_ground = true;
            let mut session = LoadedSession::new(
                world,
                origin,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            session.motion.velocity.x = 0.2;
            session
        };
        let mut stone = make(false);
        let mut ice = make(true);
        stone.update(TICK);
        ice.update(TICK);
        assert!(ice.motion.velocity.x > stone.motion.velocity.x);
        assert!((ice.player.position.x - stone.player.position.x).abs() < 1.0e-6);
    }

    #[test]
    fn tall_fence_collides_above_its_own_cell() {
        let (mut world, origin, player) = fixture();
        let fence = world.intern_state(state(
            "minecraft:oak_fence",
            &[
                ("north", "false"),
                ("south", "false"),
                ("east", "false"),
                ("west", "false"),
                ("waterlogged", "false"),
            ],
        ));
        world
            .set_block_id(origin.x + 8, 0, origin.z + 8, fence)
            .unwrap();
        let session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let mut position = Vec3::new(7.5, 1.1, 8.5);
        assert!(move_axis(
            &session.world,
            &session.shapes,
            origin,
            session.bounds,
            session.states[0],
            &mut position,
            0,
            2.0
        ));
        assert!((position.x - (8.0 + 6.0 / 16.0 - PLAYER_HALF_WIDTH)).abs() < 0.0001);
    }

    #[test]
    fn missing_and_unknown_blocks_stop_motion_and_cannot_be_placed_inside_player() {
        let (mut world, origin, player) = fixture();
        let unknown = world.intern_state(state("example:unknown", &[]));
        world
            .set_block_id(origin.x + 9, 1, origin.z + 8, unknown)
            .unwrap();
        let session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        assert_eq!(session.unsupported_states(), 1);
        assert_eq!(session.unsupported_outlines(), 1);
        let mut position = player.position;
        assert!(move_axis(
            &session.world,
            &session.shapes,
            origin,
            session.bounds,
            session.states[0],
            &mut position,
            0,
            2.0
        ));
        assert!((position.x - 8.7).abs() < 0.0001);
        let mut position = Vec3::new(15.4, 1.0, 8.5);
        assert!(move_axis(
            &session.world,
            &session.shapes,
            origin,
            session.bounds,
            session.states[0],
            &mut position,
            0,
            2.0
        ));
        assert!((position.x - 15.7).abs() < 0.0001);

        let (mut world, origin, player) = fixture();
        let stone = world.block_id(origin.x + 8, 2, origin.z + 6).unwrap();
        world
            .set_block_id(origin.x + 8, 2, origin.z + 7, stone)
            .unwrap();
        let mut session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        assert_eq!(session.snapshot().world_revision, 0);
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(crate::inventory::PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            64
        );
        assert_eq!(
            session
                .world
                .block(origin.x + 8, 2, origin.z + 8)
                .unwrap()
                .name,
            "minecraft:air"
        );
    }
}
