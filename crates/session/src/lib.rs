use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use glam::{DVec3, IVec3, Vec3};
use inventory::{
    ClickButton, InventoryMenu, InventoryPointerEvent, InventoryPointerTarget, PlayerInventory,
    PlayerSlot,
};
use native_resources::loot::BlockLootTables;
use native_resources::recipes::CraftingRecipes;
use native_resources::transforms::{
    AxeTransformers, BlockTransformers, State as TransformState, ToolKind,
};
use native_world::{Block, World};

pub mod animation;
pub mod camera;
mod equipment_attributes;
pub use equipment_attributes::ArmorValues;
pub mod equippable;
pub mod experience;
pub mod fall;
pub mod falling_block;
mod interaction;
pub mod inventory;
pub mod items;
pub mod loaded;
pub mod mining;
mod movement;
pub mod orbs;
pub mod physics;
pub mod player_data;
pub mod redstone;
pub mod respawn;
pub mod saved_falling_block;
pub mod saved_item;
pub mod saved_orb;
mod sky_lighting;
mod spawn_entities;
pub mod spawn_heightmaps;
pub mod survival;
pub mod world_spawn;

#[cfg(test)]
mod vanilla_fluid_motion;

const TICK: Duration = Duration::from_millis(50);
const MAX_TICKS_PER_UPDATE: usize = 5;
const PLAYER_HALF_WIDTH: f32 = 0.3;
const PLAYER_HEIGHT: f32 = 1.8;
const CROUCH_HEIGHT: f32 = 1.5;
const CRAWL_HEIGHT: f32 = 0.6;
pub const EYE_HEIGHT: f32 = 1.62;
pub const CROUCH_EYE_HEIGHT: f32 = 1.27;
pub const CRAWL_EYE_HEIGHT: f32 = 0.4;

pub trait GameSession {
    fn submit_command(&mut self, command: PlayerCommand);
    fn update(&mut self, delta: Duration);
    fn snapshot(&self) -> Arc<WorldSnapshot>;
    /// Fraction since the last completed tick, for visual interpolation only.
    fn partial_tick(&self) -> f32;
}

/// Pickup sounds in the order in which this session simulated them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickupSound {
    Item(DVec3),
    Experience(DVec3),
}

fn collect_pickup_sounds(
    queue: &mut VecDeque<PickupSound>,
    items: &mut items::ItemEntities,
    orbs: &mut orbs::Orbs,
) {
    for event in items
        .drain_pickup_sounds()
        .map(PickupSound::Item)
        .chain(orbs.drain_pickup_sounds().map(PickupSound::Experience))
    {
        if queue.len() == 4096 {
            queue.pop_front();
        }
        queue.push_back(event);
    }
}

/// The saved Java game mode. Other mode-specific abilities remain incomplete.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GameMode {
    #[default]
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    /// Java's legacy game-type codec maps an unknown integer to survival.
    pub fn from_legacy_id(id: i32) -> Self {
        match id {
            1 => Self::Creative,
            2 => Self::Adventure,
            3 => Self::Spectator,
            _ => Self::Survival,
        }
    }

    fn allows_basic_building(self) -> bool {
        matches!(self, Self::Survival | Self::Creative)
    }

    fn consumes_placed_item(self) -> bool {
        !matches!(self, Self::Creative)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PlayerCommand {
    /// Request respawn after a supported death transition.
    Respawn,
    Move {
        forward: f32,
        strafe: f32,
        jump: bool,
    },
    Sprint {
        pressed: bool,
        backward_pressed: bool,
    },
    Sneak(bool),
    Look {
        yaw: f32,
        pitch: f32,
    },
    Select(Block),
    SelectSlot(u8),
    /// Apply accumulated vertical wheel steps to spectator flight speed.
    AdjustFlightSpeed(i32),
    ToggleInventory,
    InventoryClick {
        slot: PlayerSlot,
        button: ClickButton,
    },
    InventoryQuickMove {
        slot: PlayerSlot,
    },
    InventoryPointer(InventoryPointerEvent),
    RecipeBookSettings {
        open: bool,
        filtering: bool,
    },
    RecipeBookPlace {
        recipe: usize,
        use_max: bool,
    },
    /// Exchange the selected hotbar item and the offhand while playing.
    SwapHands,
    /// Exchange a hovered slot with a hotbar slot or the offhand.
    InventorySwap {
        slot: PlayerSlot,
        target: PlayerSlot,
    },
    /// Q or Ctrl+Q outside the inventory.
    DropSelected {
        all: bool,
    },
    /// Q or Ctrl+Q on an inventory slot.
    InventoryThrow {
        slot: PlayerSlot,
        all: bool,
    },
    /// A left or right click outside the inventory background.
    DropCursor {
        button: ClickButton,
    },
    /// Pick the targeted supported block into the hotbar.
    PickBlock,
    /// Begin holding the attack button; `StopBreak` ends the hold.
    Break,
    StopBreak,
    /// End a held item use when the use button is released.
    StopUse,
    /// Track the physical use button so held food can be used again.
    UseHeld(bool),
    Place,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayerPose {
    #[default]
    Standing,
    Crouching,
    Crawling,
    Swimming,
}

impl PlayerPose {
    pub fn height(self) -> f32 {
        match self {
            Self::Standing => PLAYER_HEIGHT,
            Self::Crouching => CROUCH_HEIGHT,
            Self::Crawling | Self::Swimming => CRAWL_HEIGHT,
        }
    }

    pub fn eye_height(self) -> f32 {
        match self {
            Self::Standing => EYE_HEIGHT,
            Self::Crouching => CROUCH_EYE_HEIGHT,
            Self::Crawling | Self::Swimming => CRAWL_EYE_HEIGHT,
        }
    }

    /// Select the normal player pose after movement, given block-space checks.
    pub fn after_movement(
        self,
        sneak: bool,
        swimming: bool,
        standing: bool,
        crouching: bool,
        crawling: bool,
    ) -> Self {
        if !crawling {
            self
        } else if swimming {
            Self::Swimming
        } else if sneak && crouching {
            Self::Crouching
        } else if !sneak && standing {
            Self::Standing
        } else if crouching {
            Self::Crouching
        } else {
            Self::Crawling
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PlayerState {
    /// Position of the center of the player's feet.
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub vertical_velocity: f32,
    pub on_ground: bool,
    pub pose: PlayerPose,
}

/// The damage event that caused the player's death. A saved player whose
/// health is already zero has no reliable cause in the available NBT.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeathCause {
    Unknown,
    Fall,
    FallHigh,
    Stalagmite,
    Drown,
    Starve,
}

/// Server game rules involved in the player death and respawn transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeathRules {
    pub keep_inventory: bool,
    pub immediate_respawn: bool,
    pub show_death_messages: bool,
}

impl Default for DeathRules {
    fn default() -> Self {
        Self {
            keep_inventory: false,
            immediate_respawn: false,
            show_death_messages: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeathInfo {
    pub cause: DeathCause,
    pub tick: u64,
    pub score: i32,
    pub show_message: bool,
    /// The item's effects or container contents could not be resolved safely.
    pub inventory_unresolved: bool,
    /// A respawn request is waiting for world data or a safe position.
    pub respawn_pending: bool,
    pub can_respawn: bool,
}

/// A client-visible system message produced by a local game event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemMessageKind {
    MissingRespawnBlock,
    DebugFocusPause(bool),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SystemMessage {
    pub kind: SystemMessageKind,
    pub tick: u64,
}

/// Java clamps player pitch to 90 degrees in each direction.
pub const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2;

impl PlayerState {
    pub fn eye(self) -> Vec3 {
        self.position + Vec3::Y * self.pose.eye_height()
    }

    fn height(self) -> f32 {
        self.pose.height()
    }

    pub fn direction(self) -> Vec3 {
        let horizontal = self.pitch.cos();
        Vec3::new(
            self.yaw.sin() * horizontal,
            self.pitch.sin(),
            -self.yaw.cos() * horizontal,
        )
    }
}

#[derive(Clone)]
pub struct WorldSnapshot {
    pub world: SessionWorld,
    pub player: PlayerState,
    /// Camera endpoints are visual-only and never used for simulation.
    pub camera: camera::CameraMotion,
    pub game_mode: GameMode,
    pub vitals: survival::PlayerVitals,
    pub experience: experience::Experience,
    pub death: Option<DeathInfo>,
    /// None means the equipped modifiers could not be resolved safely.
    pub armor: Option<ArmorValues>,
    /// Whether the most recent fluid-contact tick placed the player's eyes in water.
    pub eyes_in_water: bool,
    /// Whether normal sprint movement was active on this tick.
    pub sprinting: bool,
    pub selected_block: Block,
    pub inventory: Arc<PlayerInventory>,
    pub first_person: animation::FirstPersonState,
    pub breaking: Option<mining::BreakingProgress>,
    pub dropped_items: Arc<[items::DroppedItem]>,
    pub falling_blocks: Arc<[falling_block::FallingBlockVisual]>,
    pub experience_orbs: Arc<[orbs::ExperienceOrb]>,
    /// Recent messages are shared between snapshots and copied only on change.
    pub system_messages: Arc<[SystemMessage]>,
    pub tick: u64,
    /// Absolute block origin for relative player and mesh coordinates.
    pub origin: IVec3,
    pub world_revision: u64,
}

#[derive(Clone)]
pub enum SessionWorld {
    Demo(World),
    Loaded(Arc<native_world::loaded::World>),
}

impl SessionWorld {
    pub fn demo(&self) -> Option<&World> {
        match self {
            Self::Demo(world) => Some(world),
            _ => None,
        }
    }

    pub fn loaded(&self) -> Option<&Arc<native_world::loaded::World>> {
        match self {
            Self::Loaded(world) => Some(world),
            _ => None,
        }
    }
}

pub struct LocalSession {
    world: World,
    player: PlayerState,
    spawn: PlayerState,
    motion: physics::MotionState,
    camera: camera::CameraMotion,
    selected_block: Block,
    inventory: Arc<PlayerInventory>,
    recipes: Option<Arc<CraftingRecipes>>,
    loot: Option<Arc<BlockLootTables>>,
    enchantments: Option<Arc<native_resources::enchantments::OxygenEnchantments>>,
    tool_transforms: [Option<Arc<BlockTransformers>>; 3],
    soil_ticks: VecDeque<(u64, IVec3, SoilKind)>,
    dropped_items: items::ItemEntities,
    experience_orbs: orbs::Orbs,
    pickup_sounds: VecDeque<PickupSound>,
    player_random: native_core::random::LegacyRandom,
    level_random: native_core::random::LegacyRandom,
    flight_speed: f32,
    flight: physics::FlightState,
    fov_effect_scale: f32,
    game_mode: GameMode,
    vitals: survival::PlayerVitals,
    experience: experience::Experience,
    death: Option<DeathInfo>,
    death_processed: bool,
    score: i32,
    armor: equipment_attributes::ArmorTracker,
    survival_rules: survival::SurvivalRules,
    food_use: survival::FoodUse,
    use_repeat: interaction::UseRepeat,
    mining: mining::Mining,
    first_person: animation::FirstPersonState,
    commands: VecDeque<PlayerCommand>,
    movement: (f32, f32, bool),
    sprint: physics::SprintState,
    sprint_window: u8,
    sneak: bool,
    previous_sneak: bool,
    fall_distance: f64,
    ground_without_support: bool,
    fall: fall::FallTracker,
    accumulated: Duration,
    tick: u64,
    snapshot: Arc<WorldSnapshot>,
}

impl LocalSession {
    /// Drain one-shot pickup sounds in simulated order after updating.
    pub fn take_pickup_sounds(&mut self) -> Vec<PickupSound> {
        self.pickup_sounds.drain(..).collect()
    }
    pub fn demo() -> Self {
        Self::new(World::demo())
    }

    pub fn new(world: World) -> Self {
        let player = PlayerState {
            position: Vec3::new(0.5, 3.0, 5.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: PlayerPose::Standing,
        };
        let inventory = Arc::new(PlayerInventory::demo());
        let camera =
            camera::CameraMotion::new(player.position.as_dvec3(), player.pose.eye_height());
        let snapshot = Arc::new(WorldSnapshot {
            world: SessionWorld::Demo(world.clone()),
            player,
            camera,
            game_mode: GameMode::default(),
            vitals: survival::PlayerVitals::default(),
            experience: experience::Experience::default(),
            death: None,
            armor: Some(ArmorValues::default()),
            eyes_in_water: false,
            sprinting: false,
            selected_block: Block::Grass,
            inventory: Arc::clone(&inventory),
            first_person: animation::FirstPersonState::default(),
            breaking: None,
            dropped_items: Arc::from([]),
            falling_blocks: Arc::from([]),
            experience_orbs: Arc::from([]),
            system_messages: Arc::from([]),
            tick: 0,
            origin: IVec3::ZERO,
            world_revision: world.revision(),
        });
        Self {
            world,
            player,
            spawn: player,
            motion: physics::MotionState {
                position: player.position.as_dvec3(),
                velocity: DVec3::ZERO,
                on_ground: player.on_ground,
                jump_delay: 0,
            },
            camera,
            selected_block: Block::Grass,
            inventory,
            recipes: None,
            loot: None,
            enchantments: None,
            tool_transforms: [None, None, None],
            soil_ticks: VecDeque::new(),
            dropped_items: items::ItemEntities::default(),
            experience_orbs: orbs::Orbs::default(),
            pickup_sounds: VecDeque::new(),
            player_random: native_core::random::LegacyRandom::new(0),
            level_random: native_core::random::LegacyRandom::new(0),
            flight_speed: 0.05,
            flight: physics::FlightState::default(),
            fov_effect_scale: 1.0,
            game_mode: GameMode::default(),
            vitals: survival::PlayerVitals::default(),
            experience: experience::Experience::default(),
            death: None,
            death_processed: false,
            score: 0,
            armor: equipment_attributes::ArmorTracker::default(),
            survival_rules: survival::SurvivalRules::default(),
            food_use: survival::FoodUse::default(),
            use_repeat: interaction::UseRepeat::default(),
            mining: mining::Mining::default(),
            first_person: animation::FirstPersonState::default(),
            commands: VecDeque::new(),
            movement: (0.0, 0.0, false),
            sprint: physics::SprintState::default(),
            sprint_window: 7,
            sneak: false,
            previous_sneak: false,
            fall_distance: 0.0,
            ground_without_support: false,
            fall: fall::FallTracker::default(),
            accumulated: Duration::ZERO,
            tick: 0,
            snapshot,
        }
    }

    pub fn pending_time(&self) -> Duration {
        self.accumulated
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

    pub fn set_player_vitals(&mut self, vitals: survival::PlayerVitals) {
        self.vitals = vitals;
        self.death = (vitals.health <= 0.0).then_some(DeathInfo {
            cause: DeathCause::Unknown,
            tick: self.tick,
            score: self.score,
            show_message: true,
            inventory_unresolved: false,
            respawn_pending: false,
            can_respawn: false,
        });
        self.death_processed = false;
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.vitals = vitals;
        snapshot.death = self.death;
    }

    pub fn set_survival_rules(&mut self, rules: survival::SurvivalRules) {
        self.survival_rules = rules;
    }

    pub fn set_player_inventory(&mut self, inventory: PlayerInventory) {
        self.mining.release();
        self.first_person.invalidate_inventory();
        self.selected_block = selected_inventory_block(&inventory);
        let mut inventory = inventory;
        if let Some(registry) = &self.enchantments {
            inventory.set_enchantments(Arc::clone(registry));
        }
        self.inventory = Arc::new(inventory);
        self.armor.invalidate();
        if let Err(error) = self.armor.refresh(&self.inventory) {
            eprintln!("player armor equipment: {error:#}");
        }
        self.snapshot = Arc::new(WorldSnapshot {
            world: SessionWorld::Demo(self.world.clone()),
            player: self.player,
            camera: self.camera,
            game_mode: self.game_mode,
            vitals: self.vitals,
            experience: self.experience,
            death: self.death,
            armor: self.armor.value(),
            eyes_in_water: false,
            sprinting: self.sprint.active,
            selected_block: self.selected_block,
            inventory: Arc::clone(&self.inventory),
            first_person: self.first_person.clone(),
            breaking: None,
            dropped_items: self.dropped_items.snapshot(),
            falling_blocks: Arc::from([]),
            experience_orbs: self.experience_orbs.snapshot(),
            system_messages: Arc::clone(&self.snapshot.system_messages),
            tick: self.tick,
            origin: IVec3::ZERO,
            world_revision: self.world.revision(),
        });
    }

    pub fn set_enchantments(
        &mut self,
        enchantments: Arc<native_resources::enchantments::OxygenEnchantments>,
    ) {
        Arc::make_mut(&mut self.inventory).set_enchantments(Arc::clone(&enchantments));
        self.armor.set_enchantments(Arc::clone(&enchantments));
        self.enchantments = Some(enchantments);
    }

    fn mining_target(&self) -> Option<mining::Candidate> {
        let hit = self
            .world
            .raycast(self.player.eye(), self.player.direction(), 5.0)?;
        let block = self.world.block(hit.position);
        let material = block.item_id().and_then(mining::Material::from_block_name);
        Some(mining::Candidate {
            position: hit.position,
            material,
        })
    }

    fn finish_breaking(&mut self, position: IVec3) {
        let Some(name) = self.world.block(position).item_id() else {
            return;
        };
        if self.set_block(position, Block::Air) {
            let hand = self
                .inventory
                .slot(PlayerSlot::Storage(self.inventory.selected()))
                .cloned();
            let properties = mining::Material::from_block_name(name)
                .and_then(|material| mining::mining_properties(material, hand.as_ref()));
            let damage = properties.map_or(0, |properties| properties.damage_per_block);
            if self.game_mode != GameMode::Creative && damage != 0 {
                Arc::make_mut(&mut self.inventory)
                    .damage_selected_tool(damage, &mut self.level_random);
            }
            self.dropped_items.drop_block(
                position,
                name,
                hand.as_ref(),
                self.game_mode,
                true,
                self.loot.as_deref(),
            );
            if self.game_mode != GameMode::Creative
                && properties.is_some_and(|p| p.correct_for_drops)
            {
                match mining::ore_experience(
                    name,
                    hand.as_ref(),
                    self.enchantments.as_deref(),
                    &mut self.level_random,
                ) {
                    Ok(Some(amount)) if amount > 0 => self
                        .experience_orbs
                        .award(position.as_dvec3() + DVec3::splat(0.5), amount),
                    Ok(_) => {}
                    Err(error) => eprintln!("ore experience for {name}: {error:#}"),
                }
            }
        }
    }

    fn use_tool(&mut self, position: IVec3, face: IVec3) -> bool {
        let Some(source) = demo_transform_state(self.world.block(position)) else {
            return false;
        };
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
                let offset = IVec3::from_array(offset);
                let position = checked_add(position, offset)?;
                demo_transform_state(self.world.block(position))
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
            // The temporary demo has no snowy surface state.
            if matches!(block, Block::Grass | Block::Podzol | Block::Mycelium)
                && result
                    .state
                    .properties
                    .get("snowy")
                    .is_some_and(|value| value == "true")
            {
                return true;
            }
            if result.update_from_neighbors
                && matches!(block, Block::DirtPath | Block::Farmland(_))
                && checked_add(position, IVec3::Y)
                    .is_none_or(|above| self.world.block(above) != Block::Air)
            {
                return true;
            }
            let Some(drops) =
                interaction_drops(&result, self.loot.as_deref(), &self.inventory, hand)
            else {
                return true;
            };
            self.set_block(position, block);
            if let Some((_, strategy)) = &result.loot {
                self.dropped_items
                    .drop_interaction(position, face, *strategy, drops, true);
            }
            if self.game_mode != GameMode::Creative && result.damage != 0 {
                Arc::make_mut(&mut self.inventory).damage_held_tool(
                    hand,
                    result.damage,
                    &mut self.level_random,
                );
            }
            let swing = if hand == PlayerSlot::Offhand {
                animation::Hand::Off
            } else {
                animation::Hand::Main
            };
            self.first_person.swing.start(swing);
            if self.inventory.slot(hand).is_some() {
                self.first_person.used(swing);
            }
            return true;
        }
        false
    }

    fn set_block(&mut self, position: IVec3, block: Block) -> bool {
        if !self.world.set_block(position, block) {
            return false;
        }
        if block.is_solid()
            && let Some(below) = checked_add(position, IVec3::NEG_Y)
            && let Some(kind) = SoilKind::from_block(self.world.block(below))
        {
            self.soil_ticks
                .push_back((self.tick.saturating_add(1), below, kind));
        }
        true
    }

    fn advance_soil_ticks(&mut self) {
        while self
            .soil_ticks
            .front()
            .is_some_and(|(due, _, _)| *due <= self.tick)
        {
            let (_, position, kind) = self.soil_ticks.pop_front().expect("checked scheduled tick");
            if SoilKind::from_block(self.world.block(position)) == Some(kind)
                && (kind == SoilKind::Path
                    || checked_add(position, IVec3::Y)
                        .is_some_and(|above| self.world.block(above).is_solid()))
            {
                self.push_entities_from_soil(position);
                self.set_block(position, Block::Dirt);
            }
        }
    }

    fn push_entities_from_soil(&mut self, position: IVec3) {
        let bounds = movement::player_bounds(self.motion.position, self.player.pose.height());
        if self.game_mode != GameMode::Spectator
            && let Some(delta) = native_world::movement::soil_expansion_push(bounds, position)
        {
            self.motion.position.y += delta;
            self.player.position = self.motion.position.as_vec3();
            self.camera.snap_position(self.motion.position);
        }
        self.dropped_items.push_from_soil_expansion(position);
        self.experience_orbs.push_from_soil_expansion(position);
    }

    fn placement_block(&mut self, position: IVec3, block: Block) -> Option<Block> {
        let above = checked_add(position, IVec3::Y)?;
        let blocked = self.world.block(above).is_solid();
        if block == Block::DirtPath && blocked {
            self.push_entities_from_soil(position);
        }
        Some(placed_soil(block, blocked))
    }

    fn advance_tick(&mut self) {
        // Java ticks the camera before ticking the player entity.
        let camera_target_eye_height = self.player.pose.eye_height();
        let camera_sprinting = self.sprint.active;
        let camera_flying = self.flight.flying;
        self.advance_soil_ticks();
        if self.death.is_some() {
            self.process_death();
            let respawn = self
                .commands
                .drain(..)
                .any(|command| matches!(command, PlayerCommand::Respawn));
            if respawn && self.death.is_some_and(|death| death.can_respawn) {
                self.respawn();
                self.tick += 1;
                return;
            }
            self.mining.release();
            self.food_use.cancel();
            self.camera.begin_bob_tick(DVec3::ZERO, false);
            self.camera.tick(
                self.motion.position,
                self.player.pose.eye_height(),
                false,
                camera_flying,
                self.fov_effect_scale,
            );
            self.dropped_items.tick(
                IVec3::ZERO,
                self.motion.position,
                self.player.pose,
                GameMode::Spectator,
                &mut self.inventory,
                |pos| native_world::collision::demo_shape(self.world.block(pos)),
                |_| 0.6,
            );
            self.tick_experience_orbs(false);
            self.tick += 1;
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
                    );
                }
                PlayerCommand::Sprint {
                    pressed,
                    backward_pressed,
                } => {
                    self.sprint.pressed = pressed;
                    self.sprint.backward_pressed = backward_pressed;
                }
                PlayerCommand::Sneak(pressed) => self.sneak = pressed,
                PlayerCommand::Look { yaw, pitch } => {
                    if yaw.is_finite() && pitch.is_finite() {
                        self.player.yaw += yaw;
                        self.player.pitch =
                            (self.player.pitch + pitch).clamp(-MAX_PITCH, MAX_PITCH);
                    }
                }
                PlayerCommand::Select(block) => select_block(&mut self.inventory, block),
                PlayerCommand::SelectSlot(slot) => {
                    Arc::make_mut(&mut self.inventory).select(slot);
                }
                PlayerCommand::AdjustFlightSpeed(steps) => {
                    if self.game_mode == GameMode::Spectator {
                        self.flight_speed = physics::adjust_flight_speed(self.flight_speed, steps);
                    }
                }
                PlayerCommand::ToggleInventory => {
                    self.mining.release();
                    self.food_use.cancel();
                    if let Some(drops) = Arc::make_mut(&mut self.inventory).toggle_with_drops() {
                        for stack in drops {
                            self.dropped_items.drop_player(
                                stack,
                                IVec3::ZERO,
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
                            IVec3::ZERO,
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
                            IVec3::ZERO,
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
                                IVec3::ZERO,
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
                            IVec3::ZERO,
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
                            IVec3::ZERO,
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
                            IVec3::ZERO,
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
                    if let Some(hit) =
                        self.world
                            .raycast(self.player.eye(), self.player.direction(), reach)
                        && let Some(item) = picked_block_item(self.world.block(hit.position))
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
                    self.first_person.swing.start(animation::Hand::Main);
                    let target = self.mining_target();
                    let hand = self
                        .inventory
                        .slot(PlayerSlot::Storage(self.inventory.selected()));
                    if let Some(position) =
                        self.mining
                            .press(target, hand, self.game_mode == GameMode::Creative)
                    {
                        self.finish_breaking(position);
                    }
                }
                PlayerCommand::Break
                    if self.game_mode != GameMode::Spectator && !self.inventory.is_open() =>
                {
                    self.first_person.swing.start(animation::Hand::Main);
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
                    let main_hand = PlayerSlot::Storage(self.inventory.selected());
                    let mut handled = false;
                    if let Some(hit) =
                        self.world
                            .raycast(self.player.eye(), self.player.direction(), 5.0)
                    {
                        let main = self
                            .inventory
                            .slot(PlayerSlot::Storage(self.inventory.selected()));
                        let off = self.inventory.slot(PlayerSlot::Offhand);
                        let bypass = self.sneak && (main.is_some() || off.is_some());
                        if self.world.block(hit.position) == Block::CraftingTable
                            && (self.game_mode == GameMode::Spectator || !bypass)
                        {
                            handled = true;
                            self.mining.release();
                            if Arc::make_mut(&mut self.inventory).open_crafting_table(hit.position)
                                && self.game_mode != GameMode::Spectator
                            {
                                self.first_person.swing.start(animation::Hand::Main);
                            }
                        } else if self.food_use.start_hand(
                            &self.inventory,
                            main_hand,
                            self.vitals,
                            self.game_mode,
                        ) || (self.game_mode.allows_basic_building()
                            && hit.adjacent.is_some_and(|adjacent| {
                                self.use_tool(hit.position, adjacent - hit.position)
                            }))
                        {
                            handled = true;
                        } else if self.game_mode.allows_basic_building()
                            && let Some((hand, block)) = placement_hand(&self.inventory)
                            && let Some(pos) = hit.adjacent
                            && self.world.block(pos) == Block::Air
                            && let Some(block) = self.placement_block(pos, block)
                            && !player_intersects_block(self.player, pos, block)
                            && self.set_block(pos, block.orient_for_face(pos - hit.position))
                        {
                            handled = true;
                            let swing_hand = if hand == PlayerSlot::Offhand {
                                animation::Hand::Off
                            } else {
                                animation::Hand::Main
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
                            PlayerSlot::Offhand,
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
            self.selected_block = selected_inventory_block(&self.inventory);
        }

        if self.mining.held()
            && !self.food_use.is_active()
            && self.game_mode.allows_basic_building()
        {
            let target = self.mining_target();
            if target.is_some() {
                self.first_person.swing.start(animation::Hand::Main);
            }
            let hand = self
                .inventory
                .slot(PlayerSlot::Storage(self.inventory.selected()));
            if let Some(position) = self.mining.tick(
                target,
                hand,
                self.game_mode == GameMode::Creative,
                self.player.on_ground,
                false,
            ) {
                self.finish_breaking(position);
            }
        }

        if let Err(error) = self.armor.refresh(&self.inventory) {
            eprintln!("player armor equipment: {error:#}");
        }
        self.food_use
            .prepare(&self.inventory, self.vitals, self.game_mode);
        let block_shape = |cell| native_world::collision::demo_shape(self.world.block(cell));
        let fits_crouching = movement::can_fit(self.motion.position, CROUCH_HEIGHT, block_shape);
        let fits_standing = movement::can_fit(self.motion.position, PLAYER_HEIGHT, block_shape);
        let moving_slowly = !self.flight.flying
            && (self.player.pose == PlayerPose::Crawling
                || (fits_crouching && (self.previous_sneak || !fits_standing)));
        self.sprint.previous_sneak = self.previous_sneak;
        self.sprint.moving_slowly = moving_slowly;
        self.sprint.insufficient_food = !self.vitals.can_sprint(self.game_mode);
        self.sprint.using_item = self.food_use.is_active();
        self.camera.begin_bob_tick(
            self.motion.velocity,
            self.motion.on_ground && self.vitals.health > 0.0,
        );
        let input = physics::MovementInput {
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
        let start_flight_jump =
            self.flight
                .update(self.game_mode, input.jump, false, self.motion.on_ground);
        if start_flight_jump {
            self.motion.start_flying_jump(input);
        }
        let flying = self.flight.flying;
        self.vitals
            .tick_peaceful_regeneration(self.survival_rules, (self.tick as i32).wrapping_add(1));
        self.vitals.tick_air(false, false, self.game_mode);
        if input.jump && self.motion.on_ground && self.motion.jump_delay <= 1 {
            self.vitals.record_jump(self.game_mode, input.sprinting);
        }
        let before_move = self.motion.position;
        let (major_collision, actual) = if self.game_mode == GameMode::Spectator {
            (
                false,
                self.motion
                    .tick_spectator(input, self.sneak, self.flight_speed),
            )
        } else if flying {
            self.motion.tick_flying(
                input,
                self.sneak,
                self.flight_speed,
                0.6,
                |position, requested, on_ground| {
                    (
                        requested,
                        movement::resolve_player(
                            position,
                            requested,
                            on_ground,
                            self.player.height(),
                            block_shape,
                        ),
                    )
                },
            )
        } else {
            self.motion
                .tick_resolved(input, 0.6, |position, requested, on_ground| {
                    let adjusted = movement::back_off(
                        position,
                        requested,
                        on_ground,
                        self.fall_distance,
                        self.player.height(),
                        self.sneak,
                        block_shape,
                    );
                    let actual = movement::resolve_player(
                        position,
                        adjusted,
                        on_ground,
                        self.player.height(),
                        block_shape,
                    );
                    (adjusted, actual)
                })
        };
        self.camera
            .finish_bob_tick(self.motion.position - before_move);
        self.vitals.record_movement(
            self.game_mode,
            actual,
            survival::MovementEnvironment {
                on_ground: self.motion.on_ground,
                sprinting: input.sprinting,
                ..Default::default()
            },
        );
        self.sprint.after_move(major_collision);
        self.flight
            .after_movement(self.game_mode, self.motion.on_ground);
        if flying {
            self.fall_distance = 0.0;
        } else {
            self.fall_distance -= f64::from(actual.y.min(0.0) as f32);
        }
        if self.motion.on_ground {
            let support = movement::supporting_block(
                self.motion.position,
                actual,
                self.ground_without_support,
                block_shape,
            );
            self.ground_without_support = support.is_none();
            let landed_on = movement::legacy_on_block(self.motion.position, support, |_| false);
            if self.game_mode != GameMode::Spectator
                && self.fall_distance > 0.0
                && matches!(self.world.block(landed_on), Block::Farmland(_))
                && farmland_trampled_by_player(
                    &mut self.level_random,
                    self.fall_distance,
                    self.player.height(),
                )
            {
                self.push_entities_from_soil(landed_on);
                self.set_block(landed_on, Block::Dirt);
            }
            if let Err(error) = self.fall.land(
                self.fall_distance,
                None,
                &self.inventory,
                self.armor.value(),
                &mut self.vitals,
                self.game_mode,
                self.survival_rules,
            ) {
                eprintln!("player fall damage: {error:#}");
            }
            if self.vitals.health <= 0.0 {
                self.death = Some(DeathInfo {
                    cause: if self.fall_distance > 5.0 {
                        DeathCause::FallHigh
                    } else {
                        DeathCause::Fall
                    },
                    tick: self.tick.saturating_add(1),
                    score: self.score,
                    show_message: true,
                    inventory_unresolved: false,
                    respawn_pending: false,
                    can_respawn: false,
                });
            }
            self.fall_distance = 0.0;
        } else {
            self.ground_without_support = false;
        }
        let block_shape = |cell| native_world::collision::demo_shape(self.world.block(cell));
        let fits_standing = movement::can_fit(self.motion.position, PLAYER_HEIGHT, block_shape);
        let fits_crouching = movement::can_fit(self.motion.position, CROUCH_HEIGHT, block_shape);
        let fits_crawling = movement::can_fit(self.motion.position, CRAWL_HEIGHT, block_shape);
        self.player.pose = if self.game_mode == GameMode::Spectator {
            PlayerPose::Standing
        } else {
            self.player.pose.after_movement(
                self.sneak && !self.flight.flying,
                false,
                fits_standing,
                fits_crouching,
                fits_crawling,
            )
        };
        self.previous_sneak = self.sneak;
        self.player.position = self.motion.position.as_vec3();
        self.player.vertical_velocity = (self.motion.velocity.y * 20.0) as f32;
        self.player.on_ground = self.motion.on_ground;
        self.camera.tick(
            self.motion.position,
            camera_target_eye_height,
            camera_sprinting,
            camera_flying,
            self.fov_effect_scale,
        );
        if let Some(InventoryMenu::CraftingTable { position }) = self.inventory.menu()
            && (self.world.block(position) != Block::CraftingTable
                || !within_crafting_range(
                    self.motion.position,
                    self.player.pose,
                    position,
                    self.game_mode,
                ))
            && let Some(drops) = Arc::make_mut(&mut self.inventory).toggle_with_drops()
        {
            for stack in drops {
                self.dropped_items.drop_player(
                    stack,
                    IVec3::ZERO,
                    self.motion.position,
                    self.player,
                    crate::items::PlayerDropOrigin::ContainerReturn,
                );
            }
        }
        let before_pickup = Arc::clone(&self.inventory);
        self.dropped_items.tick(
            IVec3::ZERO,
            self.motion.position,
            self.player.pose,
            if self.death.is_some() {
                GameMode::Spectator
            } else {
                self.game_mode
            },
            &mut self.inventory,
            block_shape,
            |_| 0.6,
        );
        update_recipe_unlocks(&mut self.inventory, &before_pickup, self.recipes.as_deref());
        self.tick_experience_orbs(self.death.is_none());
        if self.death.is_none() {
            self.food_use
                .tick(&mut self.inventory, &mut self.vitals, self.game_mode);
        }
        self.selected_block = selected_inventory_block(&self.inventory);
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
                    show_message: true,
                    inventory_unresolved: false,
                    respawn_pending: false,
                    can_respawn: false,
                });
            }
        }
        if self.death.is_some() {
            self.process_death();
        }
        self.tick += 1;
    }

    fn tick_experience_orbs(&mut self, alive: bool) {
        self.experience_orbs.tick(
            IVec3::ZERO,
            self.motion.position,
            self.player.pose,
            self.game_mode,
            alive,
            &mut self.inventory,
            &mut self.player_random,
            &mut self.experience,
            &mut self.score,
            |pos| native_world::collision::demo_shape(self.world.block(pos)),
            |_| 0.6,
        );
    }

    fn process_death(&mut self) {
        if self.death_processed {
            return;
        }
        self.death_processed = true;
        self.experience_orbs.drop_player_experience(
            self.motion.position,
            self.experience,
            false,
            self.game_mode,
        );
        self.mining.release();
        self.food_use.cancel();
        match Arc::make_mut(&mut self.inventory).take_death_drops(false) {
            Ok(drops) => {
                for stack in drops.inventory {
                    self.dropped_items.drop_death(
                        stack,
                        IVec3::ZERO,
                        self.motion.position,
                        self.player,
                    );
                }
                for stack in drops.container_overflow {
                    self.dropped_items.drop_player(
                        stack,
                        IVec3::ZERO,
                        self.motion.position,
                        self.player,
                        crate::items::PlayerDropOrigin::ContainerReturn,
                    );
                }
                if let Some(death) = &mut self.death {
                    death.can_respawn = !self.vitals.hardcore && !drops.retained_without_keep;
                }
            }
            Err(error) => {
                if let Some(death) = &mut self.death {
                    death.inventory_unresolved = true;
                }
                eprintln!("cannot safely process player death: {error:#}");
            }
        }
    }

    fn respawn(&mut self) {
        self.player = self.spawn;
        self.motion = physics::MotionState {
            position: self.spawn.position.as_dvec3(),
            velocity: DVec3::ZERO,
            on_ground: false,
            jump_delay: 0,
        };
        self.camera =
            camera::CameraMotion::new(self.motion.position, self.player.pose.eye_height());
        self.vitals = survival::PlayerVitals {
            hardcore: self.vitals.hardcore,
            ..Default::default()
        };
        self.experience = experience::Experience::default();
        self.experience_orbs.reset_player_pickup_delay();
        self.player_random = native_core::random::LegacyRandom::new(0);
        self.flight = physics::FlightState::default();
        self.flight.flying = self.game_mode == GameMode::Spectator;
        self.death = None;
        self.death_processed = false;
        self.score = 0;
        Arc::make_mut(&mut self.inventory).select(0);
        self.selected_block = selected_inventory_block(&self.inventory);
        self.movement = (0.0, 0.0, false);
        self.sprint = physics::SprintState::default();
        self.sneak = false;
        self.previous_sneak = false;
        self.fall_distance = 0.0;
        self.ground_without_support = false;
        self.fall = fall::FallTracker::default();
        self.food_use = survival::FoodUse::default();
        self.use_repeat = interaction::UseRepeat::default();
        self.mining.release();
        self.first_person = animation::FirstPersonState::default();
        self.armor.invalidate();
        if let Err(error) = self.armor.refresh(&self.inventory) {
            eprintln!("player armor equipment: {error:#}");
        }
    }
}

impl GameSession for LocalSession {
    fn submit_command(&mut self, command: PlayerCommand) {
        self.commands.push_back(command);
    }

    fn update(&mut self, delta: Duration) {
        self.accumulated = self.accumulated.saturating_add(delta);
        let mut processed = 0;
        while self.accumulated >= TICK && processed < MAX_TICKS_PER_UPDATE {
            self.accumulated -= TICK;
            self.advance_tick();
            collect_pickup_sounds(
                &mut self.pickup_sounds,
                &mut self.dropped_items,
                &mut self.experience_orbs,
            );
            processed += 1;
        }
        if processed > 0 {
            self.snapshot = Arc::new(WorldSnapshot {
                world: SessionWorld::Demo(self.world.clone()),
                player: self.player,
                camera: self.camera,
                game_mode: self.game_mode,
                vitals: self.vitals,
                experience: self.experience,
                death: self.death,
                armor: self.armor.value(),
                eyes_in_water: false,
                sprinting: self.sprint.active,
                selected_block: self.selected_block,
                inventory: Arc::clone(&self.inventory),
                first_person: self.first_person.clone(),
                breaking: self.mining.progress(),
                dropped_items: self.dropped_items.snapshot(),
                falling_blocks: Arc::from([]),
                experience_orbs: self.experience_orbs.snapshot(),
                system_messages: Arc::clone(&self.snapshot.system_messages),
                tick: self.tick,
                origin: IVec3::ZERO,
                world_revision: self.world.revision(),
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

fn selected_inventory_block(inventory: &PlayerInventory) -> Block {
    inventory_block(inventory, PlayerSlot::Storage(inventory.selected()))
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SoilKind {
    Path,
    Farmland,
}

impl SoilKind {
    fn from_block(block: Block) -> Option<Self> {
        match block {
            Block::DirtPath => Some(Self::Path),
            Block::Farmland(_) => Some(Self::Farmland),
            _ => None,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "minecraft:dirt_path" | "dirt_path" => Some(Self::Path),
            "minecraft:farmland" | "farmland" => Some(Self::Farmland),
            _ => None,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Path => "minecraft:dirt_path",
            Self::Farmland => "minecraft:farmland",
        }
    }
}

/// Farmland consumes the level random before checking the player's dimensions.
fn farmland_trampled_by_player(
    random: &mut native_core::random::LegacyRandom,
    distance: f64,
    height: f32,
) -> bool {
    let roll = random.next_float();
    f64::from(roll) < distance - 0.5 && 0.6_f32 * 0.6_f32 * height > 0.512_f32
}

fn placed_soil(block: Block, blocked_above: bool) -> Block {
    if SoilKind::from_block(block).is_some() && blocked_above {
        Block::Dirt
    } else {
        block
    }
}

fn inventory_block(inventory: &PlayerInventory, slot: PlayerSlot) -> Block {
    inventory
        .slot(slot)
        .filter(|item| item.max_count().is_some())
        .and_then(|item| Block::from_item_id(item.id()))
        .unwrap_or(Block::Air)
}

fn ordinary_tool_kind(item: &inventory::ItemStack) -> Option<ToolKind> {
    let kind = if item.id().ends_with("_axe") {
        ToolKind::Axe
    } else if item.id().ends_with("_shovel") {
        ToolKind::Shovel
    } else if item.id().ends_with("_hoe") {
        ToolKind::Hoe
    } else {
        return None;
    };
    mining::mining_properties(mining::Material::Wood, Some(item)).map(|_| kind)
}

/// Select supported tools using the same main-hand-first interaction order.
/// Other main-hand uses remain unresolved until their item behavior is known.
fn tool_hands(inventory: &PlayerInventory, sneaking: bool) -> [Option<(PlayerSlot, ToolKind)>; 2] {
    let main = PlayerSlot::Storage(inventory.selected());
    let item = inventory.slot(main);
    let main_kind = item.and_then(ordinary_tool_kind);
    if let Some(kind) = main_kind {
        // An offhand item with BLOCKS_ATTACKS (such as a shield) takes
        // precedence unless secondary use is active. Unknown components may
        // have that behavior too, so do not assume that they pass.
        if !sneaking
            && inventory
                .slot(PlayerSlot::Offhand)
                .is_some_and(|item| !inventory::ordinary_return_supported(item))
        {
            return [None, None];
        }
        let off = inventory
            .slot(PlayerSlot::Offhand)
            .and_then(ordinary_tool_kind)
            .map(|kind| (PlayerSlot::Offhand, kind));
        return [Some((main, kind)), off];
    }
    let passes = item.is_none_or(|item| {
        inventory::ordinary_return_supported(item)
            && (item.id() == "minecraft:stick"
                || item.id().ends_with("_pickaxe")
                || item.id().ends_with("_sword"))
    });
    let off = passes
        .then(|| {
            inventory
                .slot(PlayerSlot::Offhand)
                .and_then(ordinary_tool_kind)
        })
        .flatten()
        .map(|kind| (PlayerSlot::Offhand, kind));
    [None, off]
}

fn checked_add(position: IVec3, offset: IVec3) -> Option<IVec3> {
    Some(IVec3::new(
        position.x.checked_add(offset.x)?,
        position.y.checked_add(offset.y)?,
        position.z.checked_add(offset.z)?,
    ))
}

fn interaction_drops(
    result: &native_resources::transforms::Transformation,
    tables: Option<&BlockLootTables>,
    inventory: &PlayerInventory,
    hand: PlayerSlot,
) -> Option<Vec<native_resources::loot::Drop>> {
    let Some((id, _)) = &result.loot else {
        return Some(Vec::new());
    };
    let item = inventory.slot(hand);
    let enchantments = item
        .map(inventory::item_enchantments)
        .transpose()
        .ok()?
        .unwrap_or_default();
    let tool = native_resources::loot::BasicTool {
        item: item.map(inventory::ItemStack::id),
        count: item.map_or(0, inventory::ItemStack::count),
        enchantments: &enchantments,
    };
    tables?.interaction_drops(id, tool).ok()
}

fn face_name(face: IVec3) -> Option<&'static str> {
    match face {
        IVec3::X => Some("east"),
        IVec3::NEG_X => Some("west"),
        IVec3::Y => Some("up"),
        IVec3::NEG_Y => Some("down"),
        IVec3::Z => Some("south"),
        IVec3::NEG_Z => Some("north"),
        _ => None,
    }
}

fn supported_transform_state(state: &TransformState) -> Option<Block> {
    let block = Block::from_item_id(&state.name)?;
    match block {
        Block::Wood(wood, _) if state.properties.len() == 1 => {
            let axis = match state.properties.get("axis")?.as_str() {
                "x" => native_world::Axis::X,
                "y" => native_world::Axis::Y,
                "z" => native_world::Axis::Z,
                _ => return None,
            };
            Some(Block::Wood(wood, axis))
        }
        Block::Grass | Block::Podzol | Block::Mycelium
            if state.properties.len() == 1
                && matches!(
                    state.properties.get("snowy").map(String::as_str),
                    Some("true" | "false")
                ) =>
        {
            Some(block)
        }
        Block::Farmland(_) if state.properties.len() == 1 => {
            let moisture = state
                .properties
                .get("moisture")?
                .parse::<u8>()
                .ok()
                .filter(|value| *value <= 7)?;
            Some(Block::Farmland(moisture))
        }
        Block::Air
        | Block::Wood(_, _)
        | Block::Grass
        | Block::Podzol
        | Block::Mycelium
        | Block::Farmland(_) => None,
        _ if state.properties.is_empty() => Some(block),
        _ => None,
    }
}

fn demo_transform_state(block: Block) -> Option<TransformState> {
    let name = block.item_id().unwrap_or("minecraft:air").to_owned();
    let properties = match block {
        Block::Wood(_, axis) => BTreeMap::from([("axis".into(), axis.name().into())]),
        Block::Grass | Block::Podzol | Block::Mycelium => {
            BTreeMap::from([("snowy".into(), "false".into())])
        }
        Block::Farmland(moisture) => BTreeMap::from([("moisture".into(), moisture.to_string())]),
        _ => BTreeMap::new(),
    };
    Some(TransformState { name, properties })
}

/// Offhand placement is known only when the main hand is empty. Other main
/// hand items may consume, fail, or pass an interaction for item-specific reasons.
fn placement_hand(inventory: &PlayerInventory) -> Option<(PlayerSlot, Block)> {
    let main = PlayerSlot::Storage(inventory.selected());
    let hand = if inventory.slot(main).is_some() {
        main
    } else {
        PlayerSlot::Offhand
    };
    let block = inventory_block(inventory, hand);
    block.is_solid().then_some((hand, block))
}

fn inventory_click(
    inventory: &mut Arc<PlayerInventory>,
    recipes: Option<&CraftingRecipes>,
    slot: PlayerSlot,
    button: ClickButton,
    creative: bool,
) {
    let state = Arc::make_mut(inventory);
    if matches!(slot, PlayerSlot::Crafting(_) | PlayerSlot::CraftResult) {
        state.clear_ghost_recipe();
    }
    if slot == PlayerSlot::CraftResult {
        if let Some(recipes) = recipes {
            state.take_crafting_result(recipes);
        }
    } else if state.click_with_mode(slot, button, creative)
        && matches!(slot, PlayerSlot::Crafting(_))
        && let Some(recipes) = recipes
    {
        state.refresh_crafting(recipes);
    }
}

fn inventory_pointer(
    inventory: &mut Arc<PlayerInventory>,
    recipes: Option<&CraftingRecipes>,
    event: InventoryPointerEvent,
    creative: bool,
) -> Vec<(inventory::ItemStack, items::PlayerDropOrigin)> {
    fn click_target(
        inventory: &mut Arc<PlayerInventory>,
        recipes: Option<&CraftingRecipes>,
        target: InventoryPointerTarget,
        button: ClickButton,
        shift: bool,
        creative: bool,
    ) -> Vec<(inventory::ItemStack, items::PlayerDropOrigin)> {
        match target {
            InventoryPointerTarget::Slot(slot) if shift => {
                inventory_quick_move(inventory, recipes, slot, creative)
                    .into_iter()
                    .map(|stack| (stack, items::PlayerDropOrigin::ContainerReturn))
                    .collect()
            }
            InventoryPointerTarget::Slot(slot) => {
                inventory_click(inventory, recipes, slot, button, creative);
                Vec::new()
            }
            InventoryPointerTarget::Outside => Arc::make_mut(inventory)
                .drop_cursor(button)
                .into_iter()
                .map(|stack| (stack, items::PlayerDropOrigin::Direct))
                .collect(),
            InventoryPointerTarget::Background => Vec::new(),
        }
    }
    match event {
        InventoryPointerEvent::Press {
            target,
            button,
            shift,
        } => {
            if Arc::make_mut(inventory).pointer_press(target, button) {
                return click_target(inventory, recipes, target, button, shift, creative);
            }
        }
        InventoryPointerEvent::Move { slot } => Arc::make_mut(inventory).pointer_visit(slot),
        InventoryPointerEvent::Release {
            target,
            button,
            shift,
        } => {
            use inventory::PointerRelease;
            let result = Arc::make_mut(inventory).pointer_release(button);
            match result {
                PointerRelease::None => {}
                PointerRelease::Click if inventory.cursor().is_some() => {
                    return click_target(inventory, recipes, target, button, shift, creative);
                }
                PointerRelease::Slot(slot) if inventory.cursor().is_some() => {
                    return click_target(
                        inventory,
                        recipes,
                        InventoryPointerTarget::Slot(slot),
                        button,
                        false,
                        creative,
                    );
                }
                PointerRelease::Distributed { crafting: true } => {
                    let state = Arc::make_mut(inventory);
                    state.clear_ghost_recipe();
                    if let Some(recipes) = recipes {
                        state.refresh_crafting(recipes);
                    }
                }
                _ => {}
            }
        }
        InventoryPointerEvent::Cancel => Arc::make_mut(inventory).pointer_cancel(),
    }
    Vec::new()
}

fn update_recipe_unlocks(
    inventory: &mut Arc<PlayerInventory>,
    before: &Arc<PlayerInventory>,
    recipes: Option<&CraftingRecipes>,
) {
    if !Arc::ptr_eq(inventory, before)
        && let Some(recipes) = recipes
    {
        let additions = inventory.changed_recipe_unlocks(before, recipes);
        if !additions.is_empty() {
            Arc::make_mut(inventory).unlock_recipes(additions);
        }
    }
}

fn inventory_quick_move(
    inventory: &mut Arc<PlayerInventory>,
    recipes: Option<&CraftingRecipes>,
    slot: PlayerSlot,
    creative: bool,
) -> Vec<inventory::ItemStack> {
    let state = Arc::make_mut(inventory);
    if matches!(slot, PlayerSlot::Crafting(_) | PlayerSlot::CraftResult) {
        state.clear_ghost_recipe();
    }
    if slot == PlayerSlot::CraftResult {
        return recipes
            .and_then(|recipes| state.quick_crafting_result(recipes))
            .unwrap_or_default();
    }
    if state.quick_move_with_mode(slot, creative)
        && let Some(recipes) = recipes
    {
        state.refresh_crafting(recipes);
    }
    Vec::new()
}

fn within_crafting_range(feet: DVec3, pose: PlayerPose, block: IVec3, mode: GameMode) -> bool {
    let eye = feet + DVec3::Y * f64::from(pose.eye_height());
    let min = block.as_dvec3();
    let delta = eye - eye.clamp(min, min + DVec3::ONE);
    let range: f64 = if mode == GameMode::Creative { 9.0 } else { 8.5 };
    delta.length_squared() < range * range
}

fn inventory_throw(
    inventory: &mut Arc<PlayerInventory>,
    recipes: Option<&CraftingRecipes>,
    slot: PlayerSlot,
    all: bool,
    creative: bool,
) -> Option<inventory::ItemStack> {
    let state = Arc::make_mut(inventory);
    if matches!(slot, PlayerSlot::Crafting(_) | PlayerSlot::CraftResult) {
        state.clear_ghost_recipe();
    }
    let dropped = state.throw_slot_with_mode(slot, all, creative);
    if dropped.is_some()
        && matches!(slot, PlayerSlot::Crafting(_))
        && let Some(recipes) = recipes
    {
        state.refresh_crafting(recipes);
    }
    dropped
}

fn select_block(inventory: &mut Arc<PlayerInventory>, block: Block) {
    let Some(id) = block.item_id() else { return };
    if let Some(slot) = (0..9).find(|&slot| {
        inventory
            .slot(PlayerSlot::Storage(slot))
            .is_some_and(|item| item.id() == id)
    }) {
        Arc::make_mut(inventory).select(slot);
    }
}

fn picked_block_item(block: Block) -> Option<inventory::ItemStack> {
    inventory::ItemStack::new(
        block.item_id()?.into(),
        1,
        64,
        native_persistence::nbt::Tag::Compound(vec![]),
    )
}

fn player_intersects_block(player: PlayerState, position: IVec3, block: Block) -> bool {
    let min = player.position + Vec3::new(-PLAYER_HALF_WIDTH, 0.0, -PLAYER_HALF_WIDTH);
    let max = player.position + Vec3::new(PLAYER_HALF_WIDTH, player.height(), PLAYER_HALF_WIDTH);
    let block_min = position.as_vec3();
    let height = if matches!(block, Block::DirtPath | Block::Farmland(_)) {
        15.0 / 16.0
    } else {
        1.0
    };
    let block_max = block_min + Vec3::new(1.0, height, 1.0);
    min.cmplt(block_max).all() && max.cmpgt(block_min).all()
}

#[cfg(test)]
fn move_axis(world: &World, position: &mut Vec3, axis: usize, amount: f32) -> bool {
    let mut requested = Vec3::ZERO;
    requested[axis] = amount;
    let actual = movement::resolve_player(
        position.as_dvec3(),
        requested.as_dvec3(),
        false,
        PLAYER_HEIGHT,
        |cell| native_world::collision::demo_shape(world.block(cell)),
    );
    *position = (position.as_dvec3() + actual).as_vec3();
    actual[axis] != f64::from(amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pickup_sounds_keep_tick_order_across_frame_cadences() {
        use native_persistence::nbt::Tag;
        fn prepare() -> LocalSession {
            let mut session = LocalSession::demo();
            let position = session.motion.position + DVec3::new(0.0, 0.5, 0.0);
            session
                .dropped_items
                .spawn_saved(saved_item::SavedItemState {
                    source: Arc::new(Tag::Compound(vec![])),
                    uuid: None,
                    position,
                    velocity: DVec3::ZERO,
                    on_ground: false,
                    no_gravity: true,
                    age: 0,
                    pickup_delay: 2,
                    health: 5,
                    fire_ticks: 0,
                    invulnerable: false,
                    invulnerable_time: 0,
                    owner: None,
                    thrower: None,
                    stack: inventory::ItemStack::new(
                        "minecraft:stone".into(),
                        1,
                        64,
                        Tag::Compound(vec![]),
                    )
                    .unwrap(),
                });
            session.experience_orbs.spawn_saved(saved_orb::SavedOrb {
                source: Arc::new(Tag::Compound(vec![])),
                uuid: None,
                position,
                velocity: DVec3::ZERO,
                on_ground: false,
                no_gravity: true,
                age: 0,
                value: 1,
                count: 2,
                health: 5,
                fire_ticks: 0,
                invulnerable: false,
                invulnerable_time: 0,
            });
            session
        }
        let mut batched = prepare();
        let mut frequent = prepare();
        batched.update(TICK * 3);
        let batched_sounds = batched.take_pickup_sounds();
        let mut frequent_sounds = Vec::new();
        for _ in 0..3 {
            frequent.update(TICK);
            frequent_sounds.extend(frequent.take_pickup_sounds());
        }
        assert_eq!(batched_sounds, frequent_sounds);
        assert!(matches!(
            batched_sounds.as_slice(),
            [
                PickupSound::Experience(_),
                PickupSound::Item(_),
                PickupSound::Experience(_)
            ]
        ));
        assert!(batched.take_pickup_sounds().is_empty());
    }

    #[test]
    fn death_experience_is_recovered_after_respawn_at_different_frame_rates() {
        fn prepare() -> LocalSession {
            let mut session = LocalSession::demo();
            session.experience = experience::Experience {
                progress: 0.5,
                level: 15,
                total: 400,
            };
            session.score = 400;
            session.set_player_vitals(survival::PlayerVitals {
                health: 0.0,
                ..Default::default()
            });
            session
        }
        let mut a = prepare();
        let mut b = prepare();
        a.update(TICK);
        for _ in 0..5 {
            b.update(Duration::from_millis(10));
        }
        let dead = a.snapshot();
        assert_eq!(
            dead.experience_orbs
                .iter()
                .map(|orb| orb.value)
                .sum::<i32>(),
            100
        );
        for session in [&mut a, &mut b] {
            session.submit_command(PlayerCommand::Respawn);
        }
        a.update(TICK);
        for _ in 0..5 {
            b.update(Duration::from_millis(10));
        }
        assert_eq!(a.snapshot().experience, experience::Experience::default());
        for _ in 0..30 {
            a.update(TICK);
            for _ in 0..5 {
                b.update(Duration::from_millis(10));
            }
        }
        let first = a.snapshot();
        let second = b.snapshot();
        assert!(first.experience_orbs.is_empty());
        assert_eq!(first.experience.total, 100);
        assert_eq!(a.score, 100);
        assert_eq!(first.experience, second.experience);
        assert_eq!(a.score, b.score);
        assert_eq!(dead.experience_orbs.len(), 4);
        let sound_a = a.take_pickup_sounds();
        let sound_b = b.take_pickup_sounds();
        assert_eq!(sound_a.len(), 4);
        assert_eq!(sound_a, sound_b);
    }

    #[test]
    fn demo_death_drops_inventory_blocks_input_and_respawns_at_spawn() {
        fn prepare() -> LocalSession {
            let mut session = LocalSession::demo();
            session.set_player_vitals(survival::PlayerVitals {
                health: 0.0,
                ..Default::default()
            });
            session
        }
        let mut steady = prepare();
        let mut varying = prepare();
        for tick in 0..25 {
            for session in [&mut steady, &mut varying] {
                session.submit_command(PlayerCommand::Move {
                    forward: 1.0,
                    strafe: 0.0,
                    jump: true,
                });
                session.submit_command(PlayerCommand::ToggleInventory);
                if tick == 24 {
                    session.submit_command(PlayerCommand::Respawn);
                }
            }
            steady.update(TICK);
            for _ in 0..5 {
                varying.update(Duration::from_millis(10));
            }
        }
        let a = steady.snapshot();
        let b = varying.snapshot();
        assert_eq!(a.tick, 25);
        assert!(a.death.is_none());
        assert_eq!(a.vitals.health, 20.0);
        assert_eq!(a.vitals.food, 20);
        assert_eq!(a.player.position, steady.spawn.position);
        assert_eq!(a.player.position, b.player.position);
        assert!(!a.inventory.is_open());
        assert!(a.inventory.slot(PlayerSlot::Storage(0)).is_none());
        assert_eq!(a.dropped_items.len(), 3);
        assert_eq!(
            a.dropped_items
                .iter()
                .map(|item| item.position)
                .collect::<Vec<_>>(),
            b.dropped_items
                .iter()
                .map(|item| item.position)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn hardcore_demo_death_does_not_allow_survival_respawn() {
        let mut session = LocalSession::demo();
        session.set_player_vitals(survival::PlayerVitals {
            health: 0.0,
            hardcore: true,
            ..Default::default()
        });
        session.update(TICK);
        session.submit_command(PlayerCommand::Respawn);
        session.update(TICK);
        assert!(!session.snapshot().death.unwrap().can_respawn);
        assert_eq!(session.snapshot().vitals.health, 0.0);
    }

    #[test]
    fn demo_fall_death_drops_once_and_unknown_curse_preserves_inventory() {
        let mut falling = LocalSession::demo();
        falling.motion.position = DVec3::new(0.5, 11.0, 5.5);
        falling.motion.velocity = DVec3::ZERO;
        falling.set_player_vitals(survival::PlayerVitals {
            health: 1.0,
            ..Default::default()
        });
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
        assert_eq!(falling.snapshot().dropped_items.len(), 3);
        falling.update(TICK);
        assert_eq!(falling.snapshot().dropped_items.len(), 3);
        assert_eq!(
            falling.snapshot().camera.eye(0.0),
            falling.snapshot().camera.eye(1.0)
        );

        let mut unknown = LocalSession::demo();
        let components = native_persistence::nbt::Tag::Compound(vec![(
            "minecraft:enchantments".into(),
            native_persistence::nbt::Tag::Compound(vec![(
                "example:unknown".into(),
                native_persistence::nbt::Tag::Int(1),
            )]),
        )]);
        let item = inventory::ItemStack::new("minecraft:stone".into(), 1, 64, components).unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(PlayerSlot::Storage(0), Some(item.clone()));
        unknown.set_player_inventory(inventory);
        unknown.set_player_vitals(survival::PlayerVitals {
            health: 0.0,
            ..Default::default()
        });
        unknown.update(TICK);
        unknown.submit_command(PlayerCommand::Respawn);
        unknown.update(TICK);
        assert!(!unknown.snapshot().death.unwrap().can_respawn);
        assert!(unknown.snapshot().death.unwrap().inventory_unresolved);
        assert_eq!(
            unknown.snapshot().inventory.slot(PlayerSlot::Storage(0)),
            Some(&item)
        );
        assert!(unknown.snapshot().dropped_items.is_empty());
    }

    pub(super) fn test_axe_transformer() -> Arc<AxeTransformers> {
        Arc::new(AxeTransformers::parse(br#"[{"item_damage_per_use":1,
          "block_state_provider":{"type":"minecraft:rule_based","rules":[
            {"if_true":{"type":"minecraft:matching_blocks","blocks":["minecraft:oak_log","minecraft:poplar_log"]},
             "then":{"type":"minecraft:copy_properties","source":{"id":"minecraft:stripped_poplar_log"}}}
          ]}}]"#).unwrap())
    }

    pub(super) fn test_soil_transformer(kind: ToolKind) -> Arc<BlockTransformers> {
        use native_resources::packs::ResourceProvider;
        struct Source;
        impl ResourceProvider for Source {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                let bytes: Option<&[u8]> = match path {
                    "data/minecraft/block_transformer/shovel.json" => Some(br#"[{"disallowed_faces":["down"],"item_damage_per_use":1,"block_state_provider":{"type":"minecraft:rule_based","rules":[{"if_true":{"type":"minecraft:all_of","predicates":[{"type":"minecraft:matching_block_tag","tag":"minecraft:path_soils"},{"type":"minecraft:matching_block_tag","tag":"minecraft:air","offset":[0,1,0]}]},"then":{"id":"minecraft:dirt_path"}}]}}]"#),
                    "data/minecraft/block_transformer/hoe.json" => Some(br#"[{"disallowed_faces":["down"],"item_damage_per_use":1,"block_state_provider":{"type":"minecraft:rule_based","rules":[{"if_true":{"type":"minecraft:all_of","predicates":[{"type":"minecraft:matching_block_tag","tag":"minecraft:farm_soils"},{"type":"minecraft:matching_block_tag","tag":"minecraft:air","offset":[0,1,0]}]},"then":{"id":"minecraft:farmland","properties":{"moisture":"0"}}},{"if_true":{"type":"minecraft:all_of","predicates":[{"type":"minecraft:matching_blocks","blocks":"minecraft:coarse_dirt"},{"type":"minecraft:matching_block_tag","tag":"minecraft:air","offset":[0,1,0]}]},"then":{"id":"minecraft:dirt"}}]}}]"#),
                    "data/minecraft/tags/block/path_soils.json" => Some(br#"{"values":["minecraft:grass_block","minecraft:dirt","minecraft:coarse_dirt","minecraft:rooted_dirt","minecraft:podzol","minecraft:mycelium"]}"#),
                    "data/minecraft/tags/block/farm_soils.json" => Some(br#"{"values":["minecraft:grass_block","minecraft:dirt_path","minecraft:dirt"]}"#),
                    "data/minecraft/tags/block/air.json" => Some(br#"{"values":["minecraft:air","minecraft:cave_air","minecraft:void_air"]}"#),
                    _ => None,
                };
                Ok(bytes.map(<[u8]>::to_vec))
            }
        }
        Arc::new(BlockTransformers::load_for(&mut Source, kind).unwrap())
    }

    pub(super) fn installed_hoe_and_loot() -> (Arc<BlockTransformers>, Arc<BlockLootTables>) {
        use native_resources::{default_minecraft_dir, packs::ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let hoe = Arc::new(BlockTransformers::load_for(&mut resources, ToolKind::Hoe).unwrap());
        let mut loot = BlockLootTables::load(&mut resources).unwrap();
        loot.load_interactions(&mut resources, hoe.referenced_loot())
            .unwrap();
        (hoe, Arc::new(loot))
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn hoeing_rooted_dirt_uses_the_interaction_loot_and_clicked_face() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        let (hoe, loot) = installed_hoe_and_loot();
        for mode in [GameMode::Survival, GameMode::Creative] {
            for face in [
                IVec3::X,
                IVec3::NEG_X,
                IVec3::Y,
                IVec3::NEG_Y,
                IVec3::Z,
                IVec3::NEG_Z,
            ] {
                let position = IVec3::new(0, 4, 3);
                let mut world = World::demo();
                world.set_block(position, Block::RootedDirt);
                world.set_block(position + IVec3::Y, Block::Stone);
                let mut session = LocalSession::new(world);
                session.set_game_mode(mode);
                session.set_tool_transformers(ToolKind::Hoe, Arc::clone(&hoe));
                session.set_block_loot(Arc::clone(&loot));
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    PlayerSlot::Offhand,
                    ItemStack::new("minecraft:wooden_hoe".into(), 1, 1, Tag::Compound(vec![])),
                );
                session.set_player_inventory(inventory);
                assert!(session.use_tool(position, face));
                assert_eq!(session.world.block(position), Block::Dirt);
                let drops = session.dropped_items.snapshot();
                assert_eq!(drops.len(), 1);
                assert_eq!(
                    (drops[0].stack.id(), drops[0].stack.count()),
                    ("minecraft:hanging_roots", 1)
                );
                let center = position.as_dvec3() + DVec3::new(0.5, 0.375, 0.5);
                let on_face = (drops[0].position - center).dot(face.as_dvec3());
                assert!((on_face - 0.625).abs() < 1.0e-9, "{face:?}: {on_face}");
                assert_eq!(
                    session
                        .inventory
                        .slot(PlayerSlot::Offhand)
                        .unwrap()
                        .components()
                        .get("minecraft:damage")
                        .and_then(Tag::as_java_int),
                    (mode == GameMode::Survival).then_some(1)
                );
            }
        }
    }

    #[test]
    fn shovels_and_hoes_transform_soil_with_the_required_space_and_hand_order() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        for (kind, held, input, expected) in [
            (
                ToolKind::Shovel,
                "minecraft:wooden_shovel",
                Block::Grass,
                Block::DirtPath,
            ),
            (
                ToolKind::Shovel,
                "minecraft:wooden_shovel",
                Block::Dirt,
                Block::DirtPath,
            ),
            (
                ToolKind::Shovel,
                "minecraft:wooden_shovel",
                Block::CoarseDirt,
                Block::DirtPath,
            ),
            (
                ToolKind::Shovel,
                "minecraft:wooden_shovel",
                Block::RootedDirt,
                Block::DirtPath,
            ),
            (
                ToolKind::Shovel,
                "minecraft:wooden_shovel",
                Block::Podzol,
                Block::DirtPath,
            ),
            (
                ToolKind::Shovel,
                "minecraft:wooden_shovel",
                Block::Mycelium,
                Block::DirtPath,
            ),
            (
                ToolKind::Hoe,
                "minecraft:wooden_hoe",
                Block::Grass,
                Block::Farmland(0),
            ),
            (
                ToolKind::Hoe,
                "minecraft:wooden_hoe",
                Block::Dirt,
                Block::Farmland(0),
            ),
            (
                ToolKind::Hoe,
                "minecraft:wooden_hoe",
                Block::DirtPath,
                Block::Farmland(0),
            ),
            (
                ToolKind::Hoe,
                "minecraft:wooden_hoe",
                Block::CoarseDirt,
                Block::Dirt,
            ),
        ] {
            for (mode, offhand, blocked) in [
                (GameMode::Survival, false, false),
                (GameMode::Survival, true, false),
                (GameMode::Survival, false, true),
                (GameMode::Creative, false, false),
                (GameMode::Adventure, false, false),
            ] {
                let position = IVec3::new(0, 4, 3);
                let mut world = World::demo();
                world.set_block(position, input);
                if blocked {
                    world.set_block(position + IVec3::Y, Block::Stone);
                }
                let mut session = LocalSession::new(world);
                session.set_game_mode(mode);
                session.set_tool_transformers(kind, test_soil_transformer(kind));
                if offhand {
                    session.set_axe_transformers(test_axe_transformer());
                }
                let mut inventory = PlayerInventory::default();
                let hand = if offhand {
                    PlayerSlot::Offhand
                } else {
                    PlayerSlot::Storage(0)
                };
                inventory.set_slot(
                    hand,
                    ItemStack::new(held.into(), 1, 1, Tag::Compound(vec![])),
                );
                if offhand {
                    inventory.set_slot(
                        PlayerSlot::Storage(0),
                        ItemStack::new("minecraft:wooden_axe".into(), 1, 1, Tag::Compound(vec![])),
                    );
                }
                session.set_player_inventory(inventory);
                let before = session.snapshot();
                session.submit_command(PlayerCommand::Place);
                session.update(TICK);
                let after = session.snapshot();
                let changed = !blocked && mode != GameMode::Adventure;
                assert_eq!(before.world.demo().unwrap().block(position), input);
                assert_eq!(
                    after.world.demo().unwrap().block(position),
                    if changed { expected } else { input },
                    "{kind:?} {input:?} {mode:?} off={offhand} blocked={blocked}"
                );
                assert_eq!(
                    after
                        .inventory
                        .slot(hand)
                        .unwrap()
                        .components()
                        .get("minecraft:damage")
                        .and_then(Tag::as_java_int),
                    (changed && mode == GameMode::Survival).then_some(1)
                );
                if offhand {
                    assert!(
                        after
                            .inventory
                            .slot(PlayerSlot::Storage(0))
                            .unwrap()
                            .components()
                            .get("minecraft:damage")
                            .is_none()
                    );
                }
            }
        }
    }

    #[test]
    fn scheduled_soil_ticks_follow_path_and_farmland_cover_rules() {
        for soil in [Block::DirtPath, Block::Farmland(0), Block::Farmland(7)] {
            for remove_cover in [false, true] {
                let position = IVec3::new(5, 3, 5);
                let mut world = World::demo();
                world.set_block(position, soil);
                let mut session = LocalSession::new(world);
                let before = session.snapshot();
                assert!(session.set_block(position + IVec3::Y, Block::Stone));
                session.update(TICK);
                assert_eq!(
                    session.snapshot().world.demo().unwrap().block(position),
                    soil
                );
                if remove_cover {
                    assert!(session.set_block(position + IVec3::Y, Block::Air));
                }
                session.update(TICK);
                assert_eq!(
                    session.snapshot().world.demo().unwrap().block(position),
                    if remove_cover && soil != Block::DirtPath {
                        soil
                    } else {
                        Block::Dirt
                    }
                );
                assert_eq!(before.world.demo().unwrap().block(position), soil);
            }
        }
    }

    #[test]
    fn demo_creative_double_jump_enables_and_landing_disables_flight() {
        let mut session = LocalSession::demo();
        session.set_game_mode(GameMode::Creative);
        session.motion.position = DVec3::new(5.5, 8.0, 5.5);
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
    fn demo_spectator_wheel_changes_flight_on_the_next_tick() {
        let mut session = LocalSession::demo();
        session.submit_command(PlayerCommand::AdjustFlightSpeed(10));
        session.update(TICK);
        assert_eq!(session.flight_speed, 0.05);
        session.set_game_mode(GameMode::Spectator);
        session.motion.position = DVec3::new(0.0, 10.0, 0.0);
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
    fn demo_spectator_can_fly_through_blocks() {
        let mut world = World::demo();
        for y in 3..=5 {
            world.set_block(IVec3::new(5, y, 4), Block::Stone);
        }
        let mut session = LocalSession::new(world);
        session.set_game_mode(GameMode::Spectator);
        session.motion.position = DVec3::new(5.5, 3.0, 5.5);
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
        assert!(session.motion.position.z < 4.0);
        assert_eq!(session.motion.position.y, 3.0);
    }

    #[test]
    fn demo_farmland_tramples_when_player_lands() {
        let soil = IVec3::new(5, 3, 5);
        let mut world = World::demo();
        world.set_block(soil, Block::Farmland(7));
        world.set_block(soil + IVec3::Y, Block::Air);
        let mut session = LocalSession::new(world);
        session.motion.position = soil.as_dvec3() + DVec3::new(0.5, 1.0, 0.5);
        session.motion.velocity = DVec3::new(0.0, -0.2, 0.0);
        session.motion.on_ground = false;
        session.fall_distance = 2.0;
        session.update(TICK);
        assert_eq!(session.world.block(soil), Block::Dirt);
        assert_eq!(session.motion.position.y, f64::from(soil.y) + 1.0);
    }

    #[test]
    fn demo_soil_tick_lifts_player_on_the_expanded_surface() {
        let soil = IVec3::new(5, 3, 5);
        let mut world = World::demo();
        world.set_block(soil, Block::DirtPath);
        let mut session = LocalSession::new(world);
        assert!(session.set_block(soil + IVec3::Y, Block::Stone));
        session.motion.position = soil.as_dvec3() + DVec3::new(0.5, 15.0 / 16.0, 0.5);
        session.player.position = session.motion.position.as_vec3();
        session.tick = 1;
        session.advance_soil_ticks();
        assert_eq!(session.motion.position.y, f64::from(soil.y) + 1.0);
        assert_eq!(session.world.block(soil), Block::Dirt);
    }

    #[test]
    fn demo_covered_path_placement_pushes_before_collision_check() {
        let target = IVec3::new(5, 3, 5);
        let mut world = World::demo();
        world.set_block(target + IVec3::Y, Block::Stone);
        let mut session = LocalSession::new(world);
        let feet = target.as_dvec3() + DVec3::new(0.5, 15.0 / 16.0, 0.5);
        session.motion.position = feet;
        session.player.position = feet.as_vec3();
        let block = session.placement_block(target, Block::DirtPath).unwrap();
        assert_eq!(block, Block::Dirt);
        assert_eq!(session.motion.position.y, f64::from(target.y) + 1.0);
        assert!(!player_intersects_block(session.player, target, block));
    }

    #[test]
    fn placing_soil_under_a_solid_block_places_its_dirt_base() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        for item in ["minecraft:dirt_path", "minecraft:farmland"] {
            for covered in [false, true] {
                let mut world = World::demo();
                world.set_block(IVec3::new(0, 4, 3), Block::Stone);
                if covered {
                    world.set_block(IVec3::new(0, 5, 4), Block::Stone);
                }
                let mut session = LocalSession::new(world);
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    PlayerSlot::Storage(0),
                    ItemStack::new(item.into(), 2, 64, Tag::Compound(vec![])),
                );
                session.set_player_inventory(inventory);
                session.submit_command(PlayerCommand::Place);
                session.update(TICK);
                let snapshot = session.snapshot();
                assert_eq!(
                    snapshot.world.demo().unwrap().block(IVec3::new(0, 4, 4)),
                    if covered {
                        Block::Dirt
                    } else {
                        Block::from_item_id(item).unwrap()
                    }
                );
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
    fn axe_use_preserves_axis_prior_snapshots_and_hand_durability() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        use native_world::Axis;

        // mode, main item, offhand axe, starting damage, transforms, places block
        for (mode, main, off, damage, transforms, places) in [
            (
                GameMode::Survival,
                Some("minecraft:wooden_axe"),
                false,
                10,
                true,
                false,
            ),
            (GameMode::Survival, None, true, 10, true, false),
            (
                GameMode::Survival,
                Some("minecraft:stick"),
                true,
                10,
                true,
                false,
            ),
            (
                GameMode::Survival,
                Some("minecraft:stone"),
                true,
                10,
                false,
                true,
            ),
            (
                GameMode::Survival,
                Some("example:unknown"),
                true,
                10,
                false,
                false,
            ),
            (
                GameMode::Creative,
                Some("minecraft:wooden_axe"),
                false,
                10,
                true,
                false,
            ),
            (
                GameMode::Adventure,
                Some("minecraft:wooden_axe"),
                false,
                10,
                false,
                false,
            ),
            (
                GameMode::Spectator,
                Some("minecraft:wooden_axe"),
                false,
                10,
                false,
                false,
            ),
            (GameMode::Survival, None, true, 58, true, false),
        ] {
            for axis in [Axis::X, Axis::Y, Axis::Z] {
                let target = IVec3::new(0, 4, 3);
                let source = Block::Wood(
                    native_core::wood::Wood::from_id("minecraft:oak_log").unwrap(),
                    axis,
                );
                let expected = Block::Wood(
                    native_core::wood::Wood::from_id("minecraft:stripped_poplar_log").unwrap(),
                    axis,
                );
                let mut world = World::demo();
                world.set_block(target, source);
                let mut session = LocalSession::new(world);
                session.set_game_mode(mode);
                session.set_axe_transformers(test_axe_transformer());
                let mut inventory = PlayerInventory::default();
                let axe = || {
                    ItemStack::new(
                        "minecraft:wooden_axe".into(),
                        1,
                        1,
                        Tag::Compound(vec![
                            ("minecraft:damage".into(), Tag::Int(damage)),
                            (
                                "minecraft:custom_data".into(),
                                Tag::Compound(vec![("keep".into(), Tag::Int(12))]),
                            ),
                        ]),
                    )
                };
                if let Some(id) = main {
                    let stack = match id {
                        "minecraft:wooden_axe" => axe(),
                        "example:unknown" => {
                            ItemStack::unresolved(id.into(), 2, Tag::Compound(vec![]))
                        }
                        _ => ItemStack::new(id.into(), 2, 64, Tag::Compound(vec![])),
                    };
                    inventory.set_slot(PlayerSlot::Storage(0), stack);
                }
                if off {
                    inventory.set_slot(PlayerSlot::Offhand, axe());
                }
                session.set_player_inventory(inventory);
                let before = session.snapshot();
                session.submit_command(PlayerCommand::Place);
                session.update(TICK);
                let after = session.snapshot();
                assert_eq!(before.world.demo().unwrap().block(target), source);
                assert_eq!(
                    after.world.demo().unwrap().block(target),
                    if transforms { expected } else { source },
                    "{mode:?} {main:?} {off} {axis:?}"
                );
                assert_eq!(
                    after.world.demo().unwrap().block(IVec3::new(0, 4, 4)),
                    if places { Block::Stone } else { Block::Air }
                );
                let hand = if off {
                    PlayerSlot::Offhand
                } else {
                    PlayerSlot::Storage(0)
                };
                let original = before.inventory.slot(hand).unwrap();
                assert_eq!(
                    original
                        .components()
                        .get("minecraft:damage")
                        .and_then(Tag::as_java_int),
                    Some(damage)
                );
                let current = after.inventory.slot(hand);
                if transforms && mode == GameMode::Survival && damage == 58 {
                    assert!(current.is_none());
                } else {
                    let current = current.unwrap();
                    assert_eq!(
                        current
                            .components()
                            .get("minecraft:damage")
                            .and_then(Tag::as_java_int),
                        Some(damage + i32::from(transforms && mode == GameMode::Survival))
                    );
                    assert_eq!(
                        current.components().get("minecraft:custom_data"),
                        original.components().get("minecraft:custom_data")
                    );
                }
            }
        }
    }

    #[test]
    fn unresolved_offhand_does_not_let_the_main_axe_bypass_blocking_use() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        use native_world::Axis;
        for sneaking in [false, true] {
            let position = IVec3::new(0, 4, 3);
            let source = Block::Wood(
                native_core::wood::Wood::from_id("minecraft:oak_log").unwrap(),
                Axis::Y,
            );
            let stripped = Block::Wood(
                native_core::wood::Wood::from_id("minecraft:stripped_poplar_log").unwrap(),
                Axis::Y,
            );
            let mut world = World::demo();
            world.set_block(position, source);
            let mut session = LocalSession::new(world);
            session.set_axe_transformers(test_axe_transformer());
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new("minecraft:wooden_axe".into(), 1, 1, Tag::Compound(vec![])),
            );
            inventory.set_slot(
                PlayerSlot::Offhand,
                ItemStack::unresolved("minecraft:shield".into(), 1, Tag::Compound(vec![])),
            );
            session.set_player_inventory(inventory);
            session.submit_command(PlayerCommand::Sneak(sneaking));
            session.submit_command(PlayerCommand::Place);
            session.update(TICK);
            let after = session.snapshot();
            assert_eq!(
                after.world.demo().unwrap().block(position),
                if sneaking { stripped } else { source }
            );
            assert_eq!(
                after
                    .inventory
                    .slot(PlayerSlot::Storage(0))
                    .unwrap()
                    .components()
                    .get("minecraft:damage")
                    .and_then(Tag::as_java_int),
                sneaking.then_some(1)
            );
        }
    }

    #[test]
    fn inventory_drag_preserves_input_order_across_frames_and_ticks() {
        use inventory::{
            InventoryPointerEvent as Event, InventoryPointerTarget as Target, ItemStack,
        };
        use native_persistence::nbt::Tag;
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:stone".into(), 8, 64, Tag::Compound(vec![])),
        );
        inventory.toggle();
        let make = || {
            let mut session = LocalSession::demo();
            session.set_player_inventory(inventory.clone());
            session
        };
        let press = |slot| Event::Press {
            target: Target::Slot(PlayerSlot::Storage(slot)),
            button: ClickButton::Primary,
            shift: false,
        };
        let release = |slot| Event::Release {
            target: Target::Slot(PlayerSlot::Storage(slot)),
            button: ClickButton::Primary,
            shift: false,
        };
        let events = [
            press(0),
            release(0),
            press(1),
            Event::Move {
                slot: PlayerSlot::Storage(1),
            },
            Event::Move {
                slot: PlayerSlot::Storage(2),
            },
            Event::Move {
                slot: PlayerSlot::Storage(3),
            },
            release(3),
        ];
        let mut queued = make();
        let old = queued.snapshot();
        for event in events {
            queued.submit_command(PlayerCommand::InventoryPointer(event));
        }
        for _ in 0..10 {
            queued.update(Duration::from_millis(5));
        }
        let mut separated = make();
        for event in events {
            separated.submit_command(PlayerCommand::InventoryPointer(event));
            separated.update(TICK);
        }
        let after = queued.snapshot();
        assert_eq!(after.inventory, separated.snapshot().inventory);
        for index in 1..=3 {
            assert_eq!(
                after
                    .inventory
                    .slot(PlayerSlot::Storage(index))
                    .unwrap()
                    .count(),
                2
            );
        }
        assert_eq!(after.inventory.cursor().unwrap().count(), 2);
        assert_eq!(
            old.inventory.slot(PlayerSlot::Storage(0)).unwrap().count(),
            8
        );
    }

    #[test]
    fn inventory_drag_preview_and_cancellation_follow_pointer_state() {
        use inventory::{
            InventoryPointerEvent as Event, InventoryPointerTarget as Target, ItemStack,
        };
        use native_persistence::nbt::Tag;
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:stone".into(), 8, 64, Tag::Compound(vec![])),
        );
        inventory.toggle();
        let mut session = LocalSession::demo();
        session.set_player_inventory(inventory);
        let send = |session: &mut LocalSession, event| {
            session.submit_command(PlayerCommand::InventoryPointer(event));
            session.update(TICK);
        };
        let press = Event::Press {
            target: Target::Slot(PlayerSlot::Storage(0)),
            button: ClickButton::Primary,
            shift: false,
        };
        let release = Event::Release {
            target: Target::Slot(PlayerSlot::Storage(0)),
            button: ClickButton::Primary,
            shift: false,
        };
        send(&mut session, press);
        send(&mut session, release);
        send(&mut session, press);
        send(
            &mut session,
            Event::Move {
                slot: PlayerSlot::Storage(1),
            },
        );
        let snapshot = session.snapshot();
        let preview = snapshot.inventory.quick_craft_preview().unwrap();
        assert_eq!(preview.slots.len(), 1);
        assert!(preview.slots[0].item.is_none());
        assert_eq!(preview.carried.unwrap().count(), 8);
        send(
            &mut session,
            Event::Move {
                slot: PlayerSlot::Storage(2),
            },
        );
        let snapshot = session.snapshot();
        let preview = snapshot.inventory.quick_craft_preview().unwrap();
        assert_eq!(
            preview
                .slots
                .iter()
                .map(|slot| slot.item.as_ref().unwrap().count())
                .collect::<Vec<_>>(),
            [4, 4]
        );
        assert!(preview.exhausted);
        assert!(preview.carried.is_none());
        send(&mut session, Event::Cancel);
        assert!(session.snapshot().inventory.quick_craft_preview().is_none());
        assert_eq!(session.snapshot().inventory.cursor().unwrap().count(), 8);
        assert!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Storage(1))
                .is_none()
        );
    }

    #[test]
    fn changing_game_mode_updates_only_the_new_snapshot() {
        let mut session = LocalSession::demo();
        let old = session.snapshot();
        session.set_game_mode(GameMode::Spectator);
        let new = session.snapshot();
        assert_eq!(old.game_mode, GameMode::Survival);
        assert_eq!(new.game_mode, GameMode::Spectator);
        assert!(Arc::ptr_eq(&old.inventory, &new.inventory));
    }

    #[test]
    fn pick_block_obeys_game_mode() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        for mode in [GameMode::Creative, GameMode::Survival, GameMode::Spectator] {
            let mut world = World::demo();
            world.set_block(IVec3::new(0, 4, 3), Block::Stone);
            let mut session = LocalSession::new(world);
            session.set_game_mode(mode);
            let mut inventory = PlayerInventory::default();
            if mode != GameMode::Creative {
                inventory.set_slot(
                    PlayerSlot::Storage(12),
                    ItemStack::new("minecraft:stone".into(), 10, 64, Tag::Compound(vec![])),
                );
            }
            session.set_player_inventory(inventory);
            session.submit_command(PlayerCommand::PickBlock);
            session.update(TICK);
            let snapshot = session.snapshot();
            let selected = snapshot
                .inventory
                .slot(PlayerSlot::Storage(snapshot.inventory.selected()));
            match mode {
                GameMode::Creative => assert_eq!(selected.unwrap().count(), 1),
                GameMode::Survival => assert_eq!(selected.unwrap().count(), 10),
                GameMode::Spectator => assert!(selected.is_none()),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn pick_block_uses_the_mode_reach() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        for mode in [GameMode::Survival, GameMode::Creative] {
            let mut world = World::default();
            world.set_block(IVec3::new(0, 4, 0), Block::Stone);
            let mut session = LocalSession::new(world);
            session.player.position.z = 5.75;
            session.motion.position.z = 5.75;
            session.set_game_mode(mode);
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(12),
                ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])),
            );
            session.set_player_inventory(inventory);
            session.submit_command(PlayerCommand::PickBlock);
            session.update(TICK);
            let snapshot = session.snapshot();
            assert_eq!(
                snapshot.inventory.slot(PlayerSlot::Storage(0)).is_some(),
                mode == GameMode::Creative
            );
        }
    }

    #[test]
    fn first_person_animation_uses_fixed_ticks_at_different_frame_rates() {
        let mut a = LocalSession::demo();
        let mut b = LocalSession::demo();
        for session in [&mut a, &mut b] {
            session.submit_command(PlayerCommand::Break);
        }
        let mut saw_swing = false;
        for _ in 0..12 {
            for _ in 0..5 {
                a.update(Duration::from_millis(10));
            }
            b.update(TICK);
            let x = a.snapshot();
            let y = b.snapshot();
            assert_eq!(x.first_person.swing, y.first_person.swing);
            saw_swing |= x.first_person.swing.progress(1.0) > 0.1;
            assert_eq!(x.first_person.main.height, y.first_person.main.height);
            assert_eq!(x.first_person.off.height, y.first_person.off.height);
            assert_eq!(a.partial_tick(), b.partial_tick());
        }
        assert!(saw_swing);
        a.update(Duration::from_millis(25));
        b.update(Duration::from_millis(10));
        b.update(Duration::from_millis(15));
        assert_eq!(a.partial_tick(), 0.5);
        assert_eq!(a.partial_tick(), b.partial_tick());
        assert_eq!(
            a.snapshot().first_person.swing.progress(a.partial_tick()),
            b.snapshot().first_person.swing.progress(b.partial_tick())
        );
    }

    #[test]
    fn hand_and_inventory_swaps_follow_fixed_ticks_and_keep_unknown_components() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        let unknown = ItemStack::unresolved(
            "example:unresolved".into(),
            1,
            Tag::Compound(vec![("example:data".into(), Tag::Int(42))]),
        )
        .unwrap();
        let dirt = ItemStack::new("minecraft:dirt".into(), 3, 64, Tag::Compound(vec![])).unwrap();
        let make_session = || {
            let mut inv = PlayerInventory::default();
            inv.set_slot(PlayerSlot::Storage(0), Some(unknown.clone()));
            inv.set_slot(PlayerSlot::Storage(1), Some(dirt.clone()));
            let mut session = LocalSession::demo();
            session.set_player_inventory(inv);
            session
        };
        let mut slow = make_session();
        let mut fast = make_session();
        let before = slow.snapshot();
        slow.submit_command(PlayerCommand::SwapHands);
        fast.submit_command(PlayerCommand::SwapHands);
        slow.update(Duration::from_millis(49));
        assert!(Arc::ptr_eq(&before, &slow.snapshot()));
        slow.update(Duration::from_millis(1));
        fast.update(Duration::from_millis(20));
        fast.update(Duration::from_millis(30));
        for session in [&mut slow, &mut fast] {
            let swapped = session.snapshot();
            assert!(swapped.inventory.slot(PlayerSlot::Storage(0)).is_none());
            assert_eq!(swapped.inventory.slot(PlayerSlot::Offhand), Some(&unknown));
            session.submit_command(PlayerCommand::ToggleInventory);
            session.submit_command(PlayerCommand::InventorySwap {
                slot: PlayerSlot::Storage(1),
                target: PlayerSlot::Offhand,
            });
            session.update(TICK);
            let swapped = session.snapshot();
            assert_eq!(swapped.inventory.slot(PlayerSlot::Offhand), Some(&dirt));
            assert_eq!(
                swapped.inventory.slot(PlayerSlot::Storage(1)),
                Some(&unknown)
            );
        }
        assert_eq!(slow.snapshot().inventory, fast.snapshot().inventory);
        assert_eq!(
            before.inventory.slot(PlayerSlot::Storage(0)),
            Some(&unknown)
        );

        let mut spectator = make_session();
        spectator.set_game_mode(GameMode::Spectator);
        spectator.submit_command(PlayerCommand::SwapHands);
        spectator.update(TICK);
        assert_eq!(
            spectator.snapshot().inventory.slot(PlayerSlot::Storage(0)),
            Some(&unknown)
        );
        assert!(
            spectator
                .snapshot()
                .inventory
                .slot(PlayerSlot::Offhand)
                .is_none()
        );
        spectator.submit_command(PlayerCommand::ToggleInventory);
        spectator.submit_command(PlayerCommand::InventorySwap {
            slot: PlayerSlot::Storage(1),
            target: PlayerSlot::Offhand,
        });
        spectator.update(TICK);
        assert_eq!(
            spectator.snapshot().inventory.slot(PlayerSlot::Storage(1)),
            Some(&dirt)
        );
        assert!(
            spectator
                .snapshot()
                .inventory
                .slot(PlayerSlot::Offhand)
                .is_none()
        );
    }

    #[test]
    fn oversized_shortcut_drops_remainder_when_inventory_is_full() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        let mut inventory = PlayerInventory::default();
        for index in 0..36 {
            inventory.set_slot(
                PlayerSlot::Storage(index),
                ItemStack::new("minecraft:stone".into(), 64, 64, Tag::Compound(vec![])),
            );
        }
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:stone".into(), 70, 64, Tag::Compound(vec![])),
        );
        inventory.set_slot(
            PlayerSlot::Storage(10),
            ItemStack::new("minecraft:dirt".into(), 50, 64, Tag::Compound(vec![])),
        );
        let mut session = LocalSession::demo();
        session.dropped_items.set_player_uuid(Some([1, 2, 3, 4]));
        session.set_player_inventory(inventory);
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::InventorySwap {
            slot: PlayerSlot::Storage(10),
            target: PlayerSlot::Storage(0),
        });
        session.update(TICK);
        let after = session.snapshot();
        assert_eq!(
            after
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            6
        );
        assert_eq!(
            after
                .inventory
                .slot(PlayerSlot::Storage(10))
                .unwrap()
                .count(),
            64
        );
        assert_eq!(after.dropped_items.len(), 1);
        assert_eq!(after.dropped_items[0].stack.id(), "minecraft:dirt");
        assert_eq!(after.dropped_items[0].stack.count(), 50);
        assert_eq!(
            session.dropped_items.debug_created_throwers(),
            vec![Some(Some([1, 2, 3, 4]))]
        );
    }

    #[test]
    fn outside_pointer_drop_preserves_player_thrower() {
        use inventory::{
            InventoryPointerEvent as Event, InventoryPointerTarget as Target, ItemStack,
        };
        use native_persistence::nbt::Tag;
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:dirt".into(), 3, 64, Tag::Compound(vec![])),
        );
        inventory.toggle();
        let mut session = LocalSession::demo();
        session.set_player_inventory(inventory);
        session.dropped_items.set_player_uuid(Some([1, 2, 3, 4]));
        let press = |target| Event::Press {
            target,
            button: ClickButton::Primary,
            shift: false,
        };
        let release = |target| Event::Release {
            target,
            button: ClickButton::Primary,
            shift: false,
        };
        for event in [
            press(Target::Slot(PlayerSlot::Storage(0))),
            release(Target::Slot(PlayerSlot::Storage(0))),
            press(Target::Outside),
            release(Target::Outside),
        ] {
            session.submit_command(PlayerCommand::InventoryPointer(event));
        }
        session.update(TICK);
        assert_eq!(session.snapshot().dropped_items[0].stack.count(), 3);
        assert_eq!(
            session.dropped_items.debug_created_throwers(),
            vec![Some(Some([1, 2, 3, 4]))]
        );
    }

    #[test]
    fn dropping_and_closing_are_applied_on_ticks_without_losing_items() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        let mut session = LocalSession::demo();
        session.dropped_items.set_player_uuid(Some([1, 2, 3, 4]));
        let original = session.snapshot();
        session.submit_command(PlayerCommand::DropSelected { all: false });
        session.update(Duration::from_millis(49));
        assert!(Arc::ptr_eq(&original, &session.snapshot()));
        session.update(Duration::from_millis(1));
        let one = session.snapshot();
        assert_eq!(
            one.inventory.slot(PlayerSlot::Storage(0)).unwrap().count(),
            63
        );
        assert_eq!(one.dropped_items.len(), 1);
        assert_eq!(one.dropped_items[0].stack.count(), 1);
        assert_eq!(
            session.dropped_items.debug_created_throwers()[0],
            Some(Some([1, 2, 3, 4]))
        );
        assert_eq!(
            original
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            64
        );
        session.submit_command(PlayerCommand::DropSelected { all: true });
        session.update(TICK);
        let all = session.snapshot();
        assert!(all.inventory.slot(PlayerSlot::Storage(0)).is_none());
        assert_eq!(
            all.dropped_items
                .iter()
                .map(|item| item.stack.count())
                .sum::<u32>(),
            64
        );

        let mut inventory = PlayerInventory::default();
        for index in 0..36 {
            inventory.set_slot(
                PlayerSlot::Storage(index),
                ItemStack::new("minecraft:stone".into(), 64, 64, Tag::Compound(vec![])),
            );
        }
        inventory.set_slot(
            PlayerSlot::Crafting(0),
            ItemStack::new("minecraft:dirt".into(), 3, 64, Tag::Compound(vec![])),
        );
        inventory.toggle();
        session.set_player_inventory(inventory);
        session.submit_command(PlayerCommand::ToggleInventory);
        session.update(TICK);
        let closed = session.snapshot();
        assert!(!closed.inventory.is_open());
        assert!(closed.inventory.slot(PlayerSlot::Crafting(0)).is_none());
        assert_eq!(
            closed.dropped_items.last().unwrap().stack.id(),
            "minecraft:dirt"
        );
        assert_eq!(closed.dropped_items.last().unwrap().stack.count(), 3);
        assert_eq!(
            session.dropped_items.debug_created_throwers().last(),
            Some(&Some(None))
        );
    }

    #[test]
    fn dropped_items_follow_fixed_ticks_at_different_frame_cadences() {
        let mut slow = LocalSession::demo();
        let mut fast = LocalSession::demo();
        slow.submit_command(PlayerCommand::DropSelected { all: false });
        fast.submit_command(PlayerCommand::DropSelected { all: false });
        for _ in 0..10 {
            slow.update(TICK);
        }
        for _ in 0..25 {
            fast.update(Duration::from_millis(20));
        }
        let slow = slow.snapshot();
        let fast = fast.snapshot();
        assert_eq!(slow.tick, fast.tick);
        assert_eq!(
            slow.inventory.slot(PlayerSlot::Storage(0)),
            fast.inventory.slot(PlayerSlot::Storage(0))
        );
        assert_eq!(slow.dropped_items.len(), 1);
        assert_eq!(fast.dropped_items.len(), 1);
        assert_eq!(slow.dropped_items[0].stack, fast.dropped_items[0].stack);
        assert_eq!(
            slow.dropped_items[0].position,
            fast.dropped_items[0].position
        );
    }

    #[test]
    fn inventory_commands_apply_on_fixed_ticks_and_preserve_old_snapshots() {
        let mut session = LocalSession::demo();
        let original = session.snapshot();
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::InventoryClick {
            slot: PlayerSlot::Storage(0),
            button: ClickButton::Secondary,
        });
        session.update(Duration::from_millis(49));
        assert!(Arc::ptr_eq(
            &original.inventory,
            &session.snapshot().inventory
        ));
        session.update(Duration::from_millis(1));
        let after = session.snapshot();
        assert_eq!(
            original
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            64
        );
        assert_eq!(
            after
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            32
        );
        assert_eq!(after.inventory.cursor().unwrap().count(), 32);
        session.update(Duration::from_millis(50));
        assert!(Arc::ptr_eq(&after.inventory, &session.snapshot().inventory));
        session.submit_command(PlayerCommand::SelectSlot(8));
        session.update(Duration::from_millis(50));
        assert_eq!(session.snapshot().selected_block, Block::Air);
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn crafting_stone_bricks_consumes_ingredients_at_tick_boundaries() {
        use native_resources::{default_minecraft_dir, packs::ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let recipes = Arc::new(CraftingRecipes::load(&mut resources).unwrap());
        let mut world = World::demo();
        world.set_block(IVec3::new(0, 4, 3), Block::Stone);
        let mut session = LocalSession::new(world);
        session.set_crafting_recipes(recipes);
        let before = session.snapshot();
        let click = |session: &mut LocalSession, slot, button| {
            session.submit_command(PlayerCommand::InventoryClick { slot, button });
            session.update(Duration::from_millis(50));
        };
        session.submit_command(PlayerCommand::ToggleInventory);
        session.update(Duration::from_millis(50));
        click(&mut session, PlayerSlot::Storage(2), ClickButton::Primary);
        for index in 0..4 {
            click(
                &mut session,
                PlayerSlot::Crafting(index),
                ClickButton::Secondary,
            );
        }
        click(&mut session, PlayerSlot::Storage(2), ClickButton::Primary);
        let prepared = session.snapshot();
        let output = prepared.inventory.slot(PlayerSlot::CraftResult).unwrap();
        assert_eq!((output.id(), output.count()), ("minecraft:stone_bricks", 4));
        click(
            &mut session,
            PlayerSlot::CraftResult,
            ClickButton::Secondary,
        );
        let after = session.snapshot();
        let crafted = after.inventory.cursor().unwrap();
        assert_eq!(
            (crafted.id(), crafted.count()),
            ("minecraft:stone_bricks", 4)
        );
        assert!((0..4).all(|i| after.inventory.slot(PlayerSlot::Crafting(i)).is_none()));
        assert!(after.inventory.slot(PlayerSlot::CraftResult).is_none());
        assert_eq!(
            after
                .inventory
                .slot(PlayerSlot::Storage(2))
                .unwrap()
                .count(),
            60
        );
        assert_eq!(
            before
                .inventory
                .slot(PlayerSlot::Storage(2))
                .unwrap()
                .count(),
            64
        );
        click(&mut session, PlayerSlot::Storage(3), ClickButton::Primary);
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::SelectSlot(3));
        session.submit_command(PlayerCommand::Place);
        session.update(Duration::from_millis(50));
        let placed = session.snapshot();
        assert_eq!(
            placed.world.demo().unwrap().block(IVec3::new(0, 4, 4)),
            Block::StoneBricks
        );
        assert_eq!(
            placed
                .inventory
                .slot(PlayerSlot::Storage(3))
                .unwrap()
                .count(),
            3
        );
        assert_eq!(after.inventory.cursor().unwrap().count(), 4);
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn craft_place_break_and_recover_a_crafting_table() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        use native_resources::{default_minecraft_dir, packs::ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let recipes = Arc::new(CraftingRecipes::load(&mut resources).unwrap());
        let loot = Arc::new(BlockLootTables::load(&mut resources).unwrap());
        let mut world = World::demo();
        world.set_block(IVec3::new(0, 4, 3), Block::Stone);
        let mut session = LocalSession::new(world);
        session.set_crafting_recipes(recipes);
        session.set_block_loot(loot);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:oak_planks".into(), 4, 64, Tag::Compound(vec![])),
        );
        session.set_player_inventory(inventory);
        session.update(TICK * 3);
        session.submit_command(PlayerCommand::ToggleInventory);
        session.update(TICK);
        let click = |session: &mut LocalSession, slot, button| {
            session.submit_command(PlayerCommand::InventoryClick { slot, button });
            session.update(TICK);
        };
        click(&mut session, PlayerSlot::Storage(0), ClickButton::Primary);
        for i in 0..4 {
            click(
                &mut session,
                PlayerSlot::Crafting(i),
                ClickButton::Secondary,
            );
        }
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::CraftResult)
                .unwrap()
                .id(),
            "minecraft:crafting_table"
        );
        click(&mut session, PlayerSlot::CraftResult, ClickButton::Primary);
        click(&mut session, PlayerSlot::Storage(0), ClickButton::Primary);
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        let target = IVec3::new(0, 4, 4);
        assert_eq!(
            session.snapshot().world.demo().unwrap().block(target),
            Block::CraftingTable
        );
        assert!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Storage(0))
                .is_none()
        );
        session.submit_command(PlayerCommand::Break);
        let mut drop = None;
        for _ in 0..100 {
            session.update(TICK);
            let current = session.snapshot();
            if !current.dropped_items.is_empty() {
                drop = Some(current);
                break;
            }
        }
        let drop = drop.expect("crafting table should produce its vanilla loot");
        assert_eq!(drop.world.demo().unwrap().block(target), Block::Air);
        assert_eq!(drop.dropped_items[0].stack.id(), "minecraft:crafting_table");
        session.submit_command(PlayerCommand::StopBreak);
        session.motion.position = DVec3::new(0.5, 3.0, 4.0);
        for _ in 0..20 {
            session.update(TICK);
        }
        let recovered = session.snapshot();
        assert!(recovered.dropped_items.is_empty());
        assert_eq!(
            recovered
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .id(),
            "minecraft:crafting_table"
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn crafting_table_makes_three_by_three_recipes_and_shift_clicks_results() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        use native_resources::{default_minecraft_dir, packs::ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let recipes = CraftingRecipes::load(&mut resources).unwrap();
        let stack = |id: &str, count| ItemStack::new(id.into(), count, 64, Tag::Compound(vec![]));
        let mut inventory = PlayerInventory::default();
        assert!(inventory.open_crafting_table(IVec3::ZERO));
        for index in [0, 1, 2] {
            inventory.set_slot(
                PlayerSlot::Crafting(index),
                stack("minecraft:oak_planks", 3),
            );
        }
        for index in [4, 7] {
            inventory.set_slot(PlayerSlot::Crafting(index), stack("minecraft:stick", 3));
        }
        inventory.refresh_crafting(&recipes);
        assert_eq!(
            inventory.slot(PlayerSlot::CraftResult).unwrap().id(),
            "minecraft:wooden_pickaxe"
        );
        assert!(
            inventory
                .quick_crafting_result(&recipes)
                .unwrap()
                .is_empty()
        );
        for index in [8, 7, 6] {
            assert_eq!(
                inventory.slot(PlayerSlot::Storage(index)).unwrap().id(),
                "minecraft:wooden_pickaxe"
            );
        }
        assert!((0..9).all(|index| inventory.slot(PlayerSlot::Crafting(index)).is_none()));
        assert!(inventory.slot(PlayerSlot::CraftResult).is_none());

        let mut inventory = PlayerInventory::default();
        assert!(inventory.open_crafting_table(IVec3::ZERO));
        for index in 0..36 {
            inventory.set_slot(PlayerSlot::Storage(index), stack("minecraft:dirt", 64));
        }
        inventory.set_slot(PlayerSlot::Storage(8), stack("minecraft:stone_bricks", 62));
        for index in [0, 1, 3, 4] {
            inventory.set_slot(PlayerSlot::Crafting(index), stack("minecraft:stone", 2));
        }
        inventory.refresh_crafting(&recipes);
        let mut pointer_inventory = Arc::new(inventory.clone());
        let pointer_drops = inventory_pointer(
            &mut pointer_inventory,
            Some(&recipes),
            inventory::InventoryPointerEvent::Press {
                target: inventory::InventoryPointerTarget::Slot(PlayerSlot::CraftResult),
                button: ClickButton::Primary,
                shift: true,
            },
            false,
        );
        assert_eq!(pointer_drops.len(), 1);
        assert_eq!(pointer_drops[0].0.count(), 2);
        assert_eq!(pointer_drops[0].1, items::PlayerDropOrigin::ContainerReturn);
        let dropped = inventory.quick_crafting_result(&recipes).unwrap();
        assert_eq!(inventory.slot(PlayerSlot::Storage(8)).unwrap().count(), 64);
        assert_eq!(
            (dropped[0].id(), dropped[0].count()),
            ("minecraft:stone_bricks", 2)
        );
        assert_eq!(inventory.slot(PlayerSlot::CraftResult).unwrap().count(), 4);
        assert_eq!(inventory.slot(PlayerSlot::Crafting(0)).unwrap().count(), 1);

        let mut inventory = PlayerInventory::default();
        assert!(inventory.toggle());
        for index in [0, 2] {
            inventory.set_slot(
                PlayerSlot::Crafting(index),
                stack("minecraft:oak_planks", 3),
            );
        }
        assert!(
            inventory
                .quick_crafting_result(&recipes)
                .unwrap()
                .is_empty()
        );
        let sticks = inventory.slot(PlayerSlot::Storage(8)).unwrap();
        assert_eq!((sticks.id(), sticks.count()), ("minecraft:stick", 12));
        assert!((0..4).all(|index| inventory.slot(PlayerSlot::Crafting(index)).is_none()));
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn recipe_book_unlocks_vanilla_inventory_advancements_on_changed_slots() {
        use inventory::{ClickButton, ItemStack, PlayerSlot};
        use native_persistence::nbt::Tag;
        use native_resources::{
            default_minecraft_dir,
            packs::{ResourceId, ResourceStack},
        };
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let recipes = Arc::new(CraftingRecipes::load(&mut resources).unwrap());
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:oak_log".into(), 2, 64, Tag::Compound(vec![])),
        );
        let planks = ResourceId::parse("minecraft:oak_planks").unwrap();
        assert!(
            recipes
                .unlocked_by_item("minecraft:oak_log")
                .contains(&planks)
        );
        let mut session = LocalSession::demo();
        session.set_player_inventory(inventory);
        session.set_crafting_recipes(recipes);
        session.submit_command(PlayerCommand::ToggleInventory);
        session.update(TICK);
        assert!(
            !session
                .snapshot()
                .inventory
                .recipe_book()
                .known
                .contains(&planks)
        );
        session.submit_command(PlayerCommand::InventoryClick {
            slot: PlayerSlot::Storage(0),
            button: ClickButton::Secondary,
        });
        session.update(TICK);
        let after = session.snapshot();
        assert!(after.inventory.recipe_book().known.contains(&planks));
        assert!(after.inventory.recipe_book().highlighted.contains(&planks));
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn world_data_pack_can_override_the_crafting_result() {
        use native_resources::{
            datapacks::DataPackSettings, default_minecraft_dir, packs::ResourceStack,
        };
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let world = temp.path().join("world");
        let pack = world.join("datapacks/custom");
        std::fs::create_dir_all(pack.join("data/minecraft/recipe")).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        std::fs::write(pack.join("data/minecraft/recipe/stone_bricks.json"), br###"{"type":"minecraft:crafting_shaped","pattern":["##","##"],"key":{"#":"minecraft:stone"},"result":{"id":"minecraft:stone_bricks","count":2}}"###).unwrap();
        let mut resources = ResourceStack::open(&game).unwrap();
        resources
            .configure_world_data(
                &world,
                &DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/custom".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )
            .unwrap();
        let recipes = Arc::new(CraftingRecipes::load(&mut resources).unwrap());
        let mut session = LocalSession::demo();
        session.set_crafting_recipes(recipes);
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::InventoryClick {
            slot: PlayerSlot::Storage(2),
            button: ClickButton::Primary,
        });
        for index in 0..4 {
            session.submit_command(PlayerCommand::InventoryClick {
                slot: PlayerSlot::Crafting(index),
                button: ClickButton::Secondary,
            });
        }
        session.submit_command(PlayerCommand::InventoryClick {
            slot: PlayerSlot::Storage(2),
            button: ClickButton::Primary,
        });
        session.update(Duration::from_millis(50));
        let output = session.snapshot();
        assert_eq!(
            output
                .inventory
                .slot(PlayerSlot::CraftResult)
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn world_data_pack_can_override_axe_transformations() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        use native_resources::{
            datapacks::DataPackSettings, default_minecraft_dir, packs::ResourceStack,
        };
        use native_world::Axis;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let world_path = temp.path().join("world");
        let pack = world_path.join("datapacks/custom");
        std::fs::create_dir_all(pack.join("data/minecraft/block_transformer")).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        std::fs::write(pack.join("data/minecraft/block_transformer/axe.json"), br#"[{"item_damage_per_use":2,"update_from_neighbors":false,
            "block_state_provider":{"type":"minecraft:rule_based","rules":[
              {"if_true":{"type":"minecraft:matching_blocks","blocks":"minecraft:oak_log"},
               "then":{"type":"minecraft:copy_properties","source":{"id":"minecraft:stripped_spruce_log"}}}
            ]}}]"#).unwrap();
        let mut resources = ResourceStack::open(&game).unwrap();
        resources
            .configure_world_data(
                &world_path,
                &DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/custom".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )
            .unwrap();
        let axe = Arc::new(AxeTransformers::load(&mut resources).unwrap());
        let position = IVec3::new(0, 4, 3);
        let mut world = World::demo();
        world.set_block(
            position,
            Block::Wood(
                native_core::wood::Wood::from_id("minecraft:oak_log").unwrap(),
                Axis::Z,
            ),
        );
        let mut session = LocalSession::new(world);
        session.set_axe_transformers(axe);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new(
                "minecraft:wooden_axe".into(),
                1,
                1,
                Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(10))]),
            ),
        );
        session.set_player_inventory(inventory);
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        let after = session.snapshot();
        assert_eq!(
            after.world.demo().unwrap().block(position),
            Block::Wood(
                native_core::wood::Wood::from_id("minecraft:stripped_spruce_log").unwrap(),
                Axis::Z
            )
        );
        assert_eq!(
            after
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .components()
                .get("minecraft:damage")
                .and_then(Tag::as_java_int),
            Some(12)
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn world_data_pack_can_override_soil_tags_and_interaction_loot() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        use native_resources::{
            datapacks::DataPackSettings, default_minecraft_dir, packs::ResourceStack,
        };
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let world_path = temp.path().join("world");
        let pack = world_path.join("datapacks/custom");
        std::fs::create_dir_all(pack.join("data/minecraft/tags/block")).unwrap();
        std::fs::create_dir_all(pack.join("data/minecraft/loot_table/till")).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("data/minecraft/tags/block/turns_into_dirt_path.json"),
            br#"{"replace":true,"values":["minecraft:stone"]}"#,
        )
        .unwrap();
        std::fs::write(pack.join("data/minecraft/loot_table/till/rooted_dirt.json"), br#"{"type":"minecraft:block_interact","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:apple","modifier":{"type":"minecraft:set_count","count":2}}]}]}"#).unwrap();
        let mut resources = ResourceStack::open(&game).unwrap();
        resources
            .configure_world_data(
                &world_path,
                &DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/custom".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )
            .unwrap();
        let shovel =
            Arc::new(BlockTransformers::load_for(&mut resources, ToolKind::Shovel).unwrap());
        let hoe = Arc::new(BlockTransformers::load_for(&mut resources, ToolKind::Hoe).unwrap());
        let mut loot = BlockLootTables::load(&mut resources).unwrap();
        loot.load_interactions(&mut resources, hoe.referenced_loot())
            .unwrap();
        for (block, item, kind) in [
            (Block::Stone, "minecraft:wooden_shovel", ToolKind::Shovel),
            (Block::RootedDirt, "minecraft:wooden_hoe", ToolKind::Hoe),
        ] {
            let position = IVec3::new(0, 4, 3);
            let mut world = World::demo();
            world.set_block(position, block);
            let mut session = LocalSession::new(world);
            session.set_tool_transformers(ToolKind::Shovel, Arc::clone(&shovel));
            session.set_tool_transformers(ToolKind::Hoe, Arc::clone(&hoe));
            session.set_block_loot(Arc::new(loot.clone()));
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(item.into(), 1, 1, Tag::Compound(vec![])),
            );
            session.set_player_inventory(inventory);
            assert!(session.use_tool(position, IVec3::Z));
            assert_eq!(
                session.world.block(position),
                if kind == ToolKind::Shovel {
                    Block::DirtPath
                } else {
                    Block::Dirt
                }
            );
            let drops = session.dropped_items.snapshot();
            if kind == ToolKind::Hoe {
                assert_eq!(drops.len(), 1);
                assert_eq!(
                    (drops[0].stack.id(), drops[0].stack.count()),
                    ("minecraft:apple", 2)
                );
            } else {
                assert!(drops.is_empty());
            }
        }
    }

    #[test]
    fn fixed_ticks_are_independent_of_frame_cadence() {
        let mut a = LocalSession::demo();
        let mut b = LocalSession::demo();
        for session in [&mut a, &mut b] {
            session.submit_command(PlayerCommand::Move {
                forward: 1.0,
                strafe: 0.0,
                jump: false,
            });
            session.submit_command(PlayerCommand::Sprint {
                pressed: true,
                backward_pressed: false,
            });
            session.submit_command(PlayerCommand::Sneak(true));
        }
        for _ in 0..100 {
            a.update(Duration::from_millis(10));
        }
        for _ in 0..20 {
            b.update(Duration::from_millis(50));
        }
        assert_eq!(a.snapshot().tick, 20);
        assert_eq!(a.snapshot().player.position, b.snapshot().player.position);
        assert_eq!(a.snapshot().sprinting, b.snapshot().sprinting);
        assert_eq!(a.snapshot().player.pose, b.snapshot().player.pose);
    }

    #[test]
    fn sprint_input_changes_motion_and_is_visible_in_new_snapshots() {
        let mut walking = LocalSession::demo();
        let mut running = LocalSession::demo();
        for session in [&mut walking, &mut running] {
            session.update(TICK * 2);
            session.submit_command(PlayerCommand::Move {
                forward: 1.0,
                strafe: 0.0,
                jump: false,
            });
        }
        let before = running.snapshot();
        running.submit_command(PlayerCommand::Sprint {
            pressed: true,
            backward_pressed: false,
        });
        for _ in 0..5 {
            walking.update(TICK);
            running.update(TICK);
        }
        assert!(!before.sprinting);
        assert!(running.snapshot().sprinting);
        assert!(running.snapshot().player.position.z < walking.snapshot().player.position.z);
        running.submit_command(PlayerCommand::Sprint {
            pressed: false,
            backward_pressed: false,
        });
        running.update(TICK);
        assert!(running.snapshot().sprinting);
        running.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: false,
        });
        running.update(TICK);
        assert!(!running.snapshot().sprinting);
    }

    #[test]
    fn sprint_window_setting_controls_double_tap_in_demo_session() {
        for (window, expected) in [(0, false), (7, true)] {
            let mut session = LocalSession::demo();
            session.set_sprint_window(window);
            for forward in [1.0, 0.0, 1.0] {
                session.submit_command(PlayerCommand::Move {
                    forward,
                    strafe: 0.0,
                    jump: false,
                });
                session.update(TICK);
            }
            assert_eq!(session.snapshot().sprinting, expected, "window={window}");
            session.submit_command(PlayerCommand::Sprint {
                pressed: true,
                backward_pressed: false,
            });
            session.update(TICK);
            assert!(
                session.snapshot().sprinting,
                "sprint key works when window={window}"
            );
        }
    }

    #[test]
    fn sneaking_preserves_footing_at_a_platform_edge() {
        let mut world = World::demo();
        for x in 1..16 {
            for z in 3..=8 {
                for y in 0..=2 {
                    world.set_block(IVec3::new(x, y, z), Block::Air);
                }
            }
        }
        let mut sneaking = LocalSession::new(world.clone());
        let mut walking = LocalSession::new(world);
        for session in [&mut sneaking, &mut walking] {
            session.update(TICK * 2);
            session.submit_command(PlayerCommand::Move {
                forward: 0.0,
                strafe: 1.0,
                jump: false,
            });
        }
        sneaking.submit_command(PlayerCommand::Sneak(true));
        for _ in 0..100 {
            sneaking.update(TICK);
            walking.update(TICK);
        }
        let safe = sneaking.snapshot().player;
        assert_eq!(safe.pose, PlayerPose::Crouching);
        assert_eq!(safe.position.y, 3.0);
        assert!(
            safe.position.x < 1.3,
            "player walked over the edge: {safe:?}"
        );
        assert!(walking.snapshot().player.position.y < 2.0);
    }

    #[test]
    fn sneak_pose_and_input_speed_follow_the_tick_boundary() {
        let mut sneaking = LocalSession::demo();
        let mut walking = LocalSession::demo();
        for session in [&mut sneaking, &mut walking] {
            session.update(TICK * 2);
            session.submit_command(PlayerCommand::Move {
                forward: 1.0,
                strafe: 0.0,
                jump: false,
            });
        }
        let before = sneaking.snapshot();
        sneaking.submit_command(PlayerCommand::Sneak(true));
        sneaking.update(TICK);
        walking.update(TICK);
        assert_eq!(sneaking.snapshot().player.pose, PlayerPose::Crouching);
        assert_eq!(before.player.pose, PlayerPose::Standing);
        assert_eq!(
            sneaking.snapshot().player.position,
            walking.snapshot().player.position
        );
        sneaking.update(TICK);
        walking.update(TICK);
        assert!(sneaking.snapshot().player.position.z > walking.snapshot().player.position.z);
        sneaking.submit_command(PlayerCommand::Sneak(false));
        sneaking.update(TICK);
        assert_eq!(sneaking.snapshot().player.pose, PlayerPose::Standing);
    }

    #[test]
    fn overload_keeps_tick_debt_instead_of_dropping_time() {
        let mut session = LocalSession::demo();
        session.update(Duration::from_secs(1));
        assert_eq!(session.snapshot().tick, 5);
        assert_eq!(session.pending_time(), Duration::from_millis(750));
        for _ in 0..3 {
            session.update(Duration::ZERO);
        }
        assert_eq!(session.snapshot().tick, 20);
        assert_eq!(session.pending_time(), Duration::ZERO);
    }

    #[test]
    fn player_stands_on_ground_and_cannot_cross_wall() {
        let mut session = LocalSession::demo();
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: false,
        });
        for _ in 0..80 {
            session.update(TICK);
        }
        let player = session.snapshot().player;
        assert_eq!(player.position.y, 3.0);
        assert!(player.position.z >= -2.7 - 0.001);
        assert!(player.on_ground);
    }

    #[test]
    fn block_interaction_changes_only_the_new_snapshot() {
        let mut world = World::demo();
        let target = IVec3::new(0, 4, 3);
        world.set_block(target, Block::Stone);
        let mut session = LocalSession::new(world);
        session.set_game_mode(GameMode::Creative);
        let before = session.snapshot();
        session.submit_command(PlayerCommand::Break);
        session.update(TICK);
        assert_eq!(before.world.demo().unwrap().block(target), Block::Stone);
        assert_eq!(
            session.snapshot().world.demo().unwrap().block(target),
            Block::Air
        );
    }

    #[test]
    fn holding_attack_is_tick_driven_and_release_clears_the_old_progress() {
        let mut world = World::default();
        world.set_block(IVec3::new(0, 2, 5), Block::Stone);
        let target = IVec3::new(0, 4, 3);
        world.set_block(target, Block::Dirt);
        let mut sessions = [LocalSession::new(world.clone()), LocalSession::new(world)];
        for session in &mut sessions {
            session.update(TICK * 3);
            session.submit_command(PlayerCommand::Break);
            session.update(TICK);
        }
        for _ in 0..10 {
            sessions[0].update(TICK);
        }
        for _ in 0..25 {
            sessions[1].update(Duration::from_millis(20));
        }
        let before_release = sessions[0].snapshot();
        let other = sessions[1].snapshot();
        assert_eq!(before_release.breaking, other.breaking);
        assert_eq!(before_release.breaking.unwrap().position, target);
        assert_eq!(
            before_release.world.demo().unwrap().block(target),
            Block::Dirt
        );
        sessions[0].submit_command(PlayerCommand::StopBreak);
        sessions[0].update(TICK);
        assert!(sessions[0].snapshot().breaking.is_none());
        assert!(before_release.breaking.is_some());
        sessions[1].submit_command(PlayerCommand::ToggleInventory);
        sessions[1].update(TICK);
        assert!(sessions[1].snapshot().breaking.is_none());
        sessions[1].submit_command(PlayerCommand::Break);
        sessions[1].submit_command(PlayerCommand::Place);
        sessions[1].update(TICK * 3);
        assert!(sessions[1].snapshot().breaking.is_none());
        assert_eq!(
            sessions[1].snapshot().world.demo().unwrap().block(target),
            Block::Dirt
        );
    }

    #[test]
    fn demo_ore_can_be_placed_then_mined_for_experience() {
        use native_core::ore::Ore;
        use native_persistence::nbt::Tag;
        let mut world = World::demo();
        world.set_block(IVec3::new(0, 4, 3), Block::Stone);
        let mut session = LocalSession::new(world);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            inventory::ItemStack::new(Ore::Diamond.id().into(), 1, 64, Tag::Compound(vec![])),
        );
        inventory.set_slot(
            PlayerSlot::Storage(1),
            inventory::ItemStack::new("minecraft:iron_pickaxe".into(), 1, 1, Tag::Compound(vec![])),
        );
        session.set_player_inventory(inventory);
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        let target = IVec3::new(0, 4, 4);
        assert_eq!(session.world.block(target), Block::Ore(Ore::Diamond));
        assert!(session.inventory.slot(PlayerSlot::Storage(0)).is_none());
        session.submit_command(PlayerCommand::SelectSlot(1));
        session.submit_command(PlayerCommand::Break);
        for _ in 0..70 {
            session.update(TICK);
            if session.world.block(target) == Block::Air {
                break;
            }
        }
        assert_eq!(session.world.block(target), Block::Air);
        assert!(
            session.score > 0 || !session.experience_orbs.snapshot().is_empty(),
            "ore XP was neither spawned nor picked up"
        );
    }

    #[test]
    fn placement_uses_previous_ray_cell_and_cannot_intersect_player() {
        let mut world = World::demo();
        world.set_block(IVec3::new(0, 4, 3), Block::Stone);
        let mut session = LocalSession::new(world);
        session.submit_command(PlayerCommand::Select(Block::Dirt));
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        assert_eq!(
            session
                .snapshot()
                .world
                .demo()
                .unwrap()
                .block(IVec3::new(0, 4, 4)),
            Block::Dirt
        );

        let mut world = World::demo();
        world.set_block(IVec3::new(0, 4, 4), Block::Stone);
        let mut session = LocalSession::new(world);
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        assert_eq!(
            session
                .snapshot()
                .world
                .demo()
                .unwrap()
                .block(IVec3::new(0, 4, 5)),
            Block::Air
        );
    }

    #[test]
    fn holding_use_repeats_at_four_ticks_and_stops_on_release() {
        let mut world = World::demo();
        world.set_block(IVec3::new(0, 4, 1), Block::Stone);
        let mut session = LocalSession::new(world);
        session.submit_command(PlayerCommand::Select(Block::Dirt));
        session.submit_command(PlayerCommand::UseHeld(true));
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        assert_eq!(session.world.block(IVec3::new(0, 4, 2)), Block::Dirt);
        session.update(TICK * 3);
        assert_eq!(session.world.block(IVec3::new(0, 4, 3)), Block::Air);
        session.update(TICK);
        assert_eq!(session.world.block(IVec3::new(0, 4, 3)), Block::Dirt);
        session.submit_command(PlayerCommand::StopUse);
        session.update(TICK * 4);
        assert_eq!(session.world.block(IVec3::new(0, 4, 4)), Block::Air);
    }

    #[test]
    fn held_use_waits_while_destroying_a_block() {
        let mut world = World::demo();
        world.set_block(IVec3::new(0, 4, 3), Block::Stone);
        let mut session = LocalSession::new(world);
        session.submit_command(PlayerCommand::Select(Block::Dirt));
        session.submit_command(PlayerCommand::Break);
        session.submit_command(PlayerCommand::UseHeld(true));
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        assert_eq!(session.world.block(IVec3::new(0, 4, 4)), Block::Air);
        session.submit_command(PlayerCommand::StopBreak);
        session.update(TICK);
        assert_eq!(session.world.block(IVec3::new(0, 4, 4)), Block::Dirt);
    }

    #[test]
    fn crafting_table_opens_before_item_use_unless_sneaking_with_an_item() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        for mode in [
            GameMode::Survival,
            GameMode::Creative,
            GameMode::Adventure,
            GameMode::Spectator,
        ] {
            for (sneaking, held) in [(false, false), (false, true), (true, false), (true, true)] {
                let mut world = World::demo();
                let table = IVec3::new(0, 4, 3);
                world.set_block(table, Block::CraftingTable);
                let mut session = LocalSession::new(world);
                session.set_game_mode(mode);
                let mut inventory = PlayerInventory::default();
                if held {
                    inventory.set_slot(
                        PlayerSlot::Storage(0),
                        ItemStack::new("minecraft:dirt".into(), 2, 64, Tag::Compound(vec![])),
                    );
                }
                session.set_player_inventory(inventory);
                session.submit_command(PlayerCommand::Sneak(sneaking));
                session.submit_command(PlayerCommand::Place);
                session.update(TICK);
                let after = session.snapshot();
                let opens = mode == GameMode::Spectator || !sneaking || !held;
                assert_eq!(
                    after.inventory.menu(),
                    opens.then_some(InventoryMenu::CraftingTable { position: table }),
                    "{mode:?} sneak={sneaking} held={held}"
                );
                if opens && held {
                    assert_eq!(
                        after
                            .inventory
                            .slot(PlayerSlot::Storage(0))
                            .unwrap()
                            .count(),
                        2
                    );
                }
                if sneaking && held && mode == GameMode::Survival {
                    assert_eq!(
                        after.world.demo().unwrap().block(IVec3::new(0, 4, 4)),
                        Block::Dirt
                    );
                    assert_eq!(
                        after
                            .inventory
                            .slot(PlayerSlot::Storage(0))
                            .unwrap()
                            .count(),
                        1
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_crafting_table_returns_or_drops_its_inputs() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        let table = IVec3::new(0, 4, 3);
        let mut world = World::demo();
        world.set_block(table, Block::CraftingTable);
        let mut session = LocalSession::new(world);
        let mut inventory = PlayerInventory::default();
        for i in 0..36 {
            inventory.set_slot(
                PlayerSlot::Storage(i),
                ItemStack::new("minecraft:stone".into(), 64, 64, Tag::Compound(vec![])),
            );
        }
        session.set_player_inventory(inventory);
        session.submit_command(PlayerCommand::Place);
        session.update(TICK);
        assert!(session.snapshot().inventory.is_open());
        Arc::make_mut(&mut session.inventory).set_slot(
            PlayerSlot::Crafting(8),
            ItemStack::new("minecraft:dirt".into(), 2, 64, Tag::Compound(vec![])),
        );
        session.world.set_block(table, Block::Air);
        session.update(TICK);
        let after = session.snapshot();
        assert!(!after.inventory.is_open());
        assert_eq!(
            (
                after.dropped_items[0].stack.id(),
                after.dropped_items[0].stack.count()
            ),
            ("minecraft:dirt", 2)
        );
        assert!(after.inventory.slot(PlayerSlot::Crafting(0)).is_none());
    }

    #[test]
    fn crafting_table_range_uses_eye_to_block_box_and_creative_bonus() {
        let feet = |x: f64| DVec3::new(x, 0.5 - f64::from(PlayerPose::Standing.eye_height()), 0.5);
        assert!(within_crafting_range(
            feet(9.499),
            PlayerPose::Standing,
            IVec3::ZERO,
            GameMode::Survival
        ));
        assert!(!within_crafting_range(
            feet(9.5),
            PlayerPose::Standing,
            IVec3::ZERO,
            GameMode::Survival
        ));
        assert!(within_crafting_range(
            feet(9.5),
            PlayerPose::Standing,
            IVec3::ZERO,
            GameMode::Creative
        ));
        assert!(!within_crafting_range(
            feet(10.0),
            PlayerPose::Standing,
            IVec3::ZERO,
            GameMode::Creative
        ));
    }

    #[test]
    fn crafted_planks_and_table_can_be_placed_in_the_demo() {
        use inventory::ItemStack;
        use native_core::wood::Planks;
        use native_persistence::nbt::Tag;
        for block in Planks::ALL
            .into_iter()
            .map(Block::Planks)
            .chain([Block::CraftingTable])
        {
            for (mode, expected_count) in [
                (GameMode::Survival, Some(1)),
                (GameMode::Creative, Some(2)),
                (GameMode::Adventure, Some(2)),
            ] {
                let mut world = World::demo();
                world.set_block(IVec3::new(0, 4, 3), Block::Stone);
                let mut session = LocalSession::new(world);
                session.set_game_mode(mode);
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    PlayerSlot::Storage(0),
                    ItemStack::new(
                        block.item_id().unwrap().into(),
                        2,
                        64,
                        Tag::Compound(vec![]),
                    ),
                );
                session.set_player_inventory(inventory);
                let before = session.snapshot();
                session.submit_command(PlayerCommand::Place);
                session.update(TICK);
                let after = session.snapshot();
                let target = IVec3::new(0, 4, 4);
                assert_eq!(before.world.demo().unwrap().block(target), Block::Air);
                assert_eq!(
                    after.world.demo().unwrap().block(target),
                    if mode == GameMode::Adventure {
                        Block::Air
                    } else {
                        block
                    }
                );
                assert_eq!(
                    after
                        .inventory
                        .slot(PlayerSlot::Storage(0))
                        .map(ItemStack::count),
                    expected_count
                );
            }
        }
    }

    #[test]
    fn basic_placement_uses_offhand_only_when_main_hand_is_empty() {
        use inventory::ItemStack;
        use native_persistence::nbt::Tag;
        for (mode, main, expected, main_count, off_count) in [
            (GameMode::Survival, None, Block::Dirt, None, 1),
            (GameMode::Creative, None, Block::Dirt, None, 2),
            (
                GameMode::Survival,
                Some("minecraft:stone"),
                Block::Stone,
                Some(1),
                2,
            ),
            (
                GameMode::Survival,
                Some("example:unknown"),
                Block::Air,
                Some(2),
                2,
            ),
            (GameMode::Spectator, None, Block::Air, None, 2),
        ] {
            let mut world = World::demo();
            world.set_block(IVec3::new(0, 4, 3), Block::Stone);
            let mut session = LocalSession::new(world);
            session.set_game_mode(mode);
            let mut inventory = PlayerInventory::default();
            if let Some(id) = main {
                let item = if id.starts_with("example:") {
                    ItemStack::unresolved(id.into(), 2, Tag::Compound(vec![]))
                } else {
                    ItemStack::new(id.into(), 2, 64, Tag::Compound(vec![]))
                };
                inventory.set_slot(PlayerSlot::Storage(0), item);
            }
            inventory.set_slot(
                PlayerSlot::Offhand,
                ItemStack::new("minecraft:dirt".into(), 2, 64, Tag::Compound(vec![])),
            );
            session.set_player_inventory(inventory);
            session.submit_command(PlayerCommand::Place);
            session.update(TICK);
            let after = session.snapshot();
            assert_eq!(
                after.first_person.swing.hand(),
                if expected == Block::Air {
                    None
                } else if main.is_none() {
                    Some(animation::Hand::Off)
                } else {
                    Some(animation::Hand::Main)
                }
            );
            assert_eq!(
                after.world.demo().unwrap().block(IVec3::new(0, 4, 4)),
                expected
            );
            assert_eq!(
                after
                    .inventory
                    .slot(PlayerSlot::Storage(0))
                    .map(ItemStack::count),
                main_count
            );
            assert_eq!(
                after.inventory.slot(PlayerSlot::Offhand).unwrap().count(),
                off_count
            );
        }
    }

    #[test]
    fn jump_rises_and_swept_collision_cannot_skip_the_ground() {
        let mut session = LocalSession::demo();
        session.update(TICK * 2);
        assert!(session.snapshot().player.on_ground);
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: true,
        });
        session.update(TICK);
        assert!(session.snapshot().player.position.y > 3.0);
        let mut position = Vec3::new(0.5, 15.0, 5.5);
        assert!(move_axis(&session.world, &mut position, 1, -20.0));
        assert_eq!(position.y, 3.0);
    }
}
