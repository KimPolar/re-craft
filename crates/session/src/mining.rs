//! Basic block-breaking progress for supported ordinary items and blocks.

use glam::IVec3;

use crate::inventory::{ItemStack, ordinary_item_default_max, ordinary_return_supported};

/// Vanilla 26.3's ore experience providers, including constant-zero providers.
pub fn ore_experience_range(name: &str) -> Option<(i32, i32)> {
    let name = name.strip_prefix("minecraft:")?;
    let name = name.strip_prefix("deepslate_").unwrap_or(name);
    Some(match name {
        "coal_ore" => (0, 2),
        "diamond_ore" | "emerald_ore" => (3, 7),
        "lapis_ore" | "nether_quartz_ore" => (2, 5),
        "redstone_ore" => (1, 5),
        "nether_gold_ore" => (0, 1),
        "iron_ore" | "gold_ore" | "copper_ore" => (0, 0),
        _ => return None,
    })
}

/// Sample ore XP before applying the held stack's active data-pack effects.
pub fn ore_experience(
    name: &str,
    item: Option<&ItemStack>,
    registry: Option<&native_resources::enchantments::OxygenEnchantments>,
    random: &mut native_core::random::LegacyRandom,
) -> anyhow::Result<Option<i32>> {
    use anyhow::{Context, ensure};
    let Some((min, max)) = ore_experience_range(name) else {
        return Ok(None);
    };
    let mut next = random.clone();
    let amount = if min == max {
        min
    } else {
        min + next
            .next_int_bound(max - min + 1)
            .context("invalid experience range")?
    };
    let mut value = amount as f32;
    let mut effects = 0;
    if let Some(item) = item {
        for (id, level) in crate::inventory::item_enchantments(item)? {
            let registry = registry.context("active enchantment registry is unavailable")?;
            let (result, active) =
                registry.apply_block_experience(&id, level, item.id(), value, &mut || {
                    next.next_float()
                })?;
            effects += usize::from(active);
            ensure!(
                effects <= 1,
                "ordering of multiple block experience enchantments is not resolved"
            );
            value = result;
        }
    }
    *random = next;
    Ok(Some(value as i32))
}

/// Blocks whose destruction requires Java's game-master permission as well
/// as creative abilities. The session must supply the player's permission.
pub fn requires_game_master_permission(name: &str) -> bool {
    matches!(
        name,
        "minecraft:command_block"
            | "minecraft:repeating_command_block"
            | "minecraft:chain_command_block"
            | "minecraft:structure_block"
            | "minecraft:jigsaw"
            | "minecraft:test_block"
            | "minecraft:test_instance_block"
    )
}

/// The bare-hand behavior of the basic blocks currently understood by the client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StairTool {
    Axe,
    Pickaxe,
    StonePickaxe,
    Shears,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Material {
    Grass,
    Dirt,
    Sand,
    Gravel,
    Snow,
    SnowBlock,
    HoneyBlock,
    DirtPath,
    Stone,
    StoneBricks,
    StoneButton,
    WoodButton,
    RedstoneLamp,
    RedstoneBlock,
    Lodestone,
    Lever,
    Cobblestone,
    /// Vanilla ore hardness and the minimum harvest tier: wood, stone or iron.
    Ore {
        deepslate: bool,
        tier: u8,
    },
    Wood,
    Planks,
    CraftingTable,
    Chest,
    Banner,
    Stair {
        hardness_tenths: u8,
        tool: StairTool,
    },
    Slab {
        hardness_tenths: u8,
        tool: StairTool,
    },
    Unbreakable,
}

impl Material {
    pub fn from_block_name(name: &str) -> Option<Self> {
        let banner = name
            .strip_suffix("_wall_banner")
            .map(|prefix| format!("{prefix}_banner"));
        if crate::inventory::is_banner_item(banner.as_deref().unwrap_or(name)) {
            return Some(Self::Banner);
        }
        if let Some(material) = crate::slab_mining::material(name) {
            return Some(material);
        }
        if let Some(material) = crate::stair_mining::material(name) {
            return Some(material);
        }
        match name {
            "minecraft:grass_block" | "minecraft:mycelium" | "minecraft:farmland" => {
                Some(Self::Grass)
            }
            "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:rooted_dirt"
            | "minecraft:podzol" => Some(Self::Dirt),
            "minecraft:dirt_path" => Some(Self::DirtPath),
            "minecraft:sand" | "minecraft:red_sand" => Some(Self::Sand),
            "minecraft:gravel" => Some(Self::Gravel),
            "minecraft:snow" => Some(Self::Snow),
            "minecraft:snow_block" => Some(Self::SnowBlock),
            "minecraft:honey_block" => Some(Self::HoneyBlock),
            "minecraft:stone" => Some(Self::Stone),
            "minecraft:stone_bricks" => Some(Self::StoneBricks),
            "minecraft:stone_button" => Some(Self::StoneButton),
            "minecraft:redstone_lamp" => Some(Self::RedstoneLamp),
            "minecraft:redstone_block" => Some(Self::RedstoneBlock),
            "minecraft:lodestone" => Some(Self::Lodestone),
            "minecraft:lever" => Some(Self::Lever),
            "minecraft:cobblestone" => Some(Self::Cobblestone),
            "minecraft:coal_ore" | "minecraft:nether_gold_ore" | "minecraft:nether_quartz_ore" => {
                Some(Self::Ore {
                    deepslate: false,
                    tier: 0,
                })
            }
            "minecraft:iron_ore" | "minecraft:copper_ore" | "minecraft:lapis_ore" => {
                Some(Self::Ore {
                    deepslate: false,
                    tier: 1,
                })
            }
            "minecraft:gold_ore"
            | "minecraft:diamond_ore"
            | "minecraft:emerald_ore"
            | "minecraft:redstone_ore" => Some(Self::Ore {
                deepslate: false,
                tier: 2,
            }),
            "minecraft:deepslate_coal_ore" => Some(Self::Ore {
                deepslate: true,
                tier: 0,
            }),
            "minecraft:deepslate_iron_ore"
            | "minecraft:deepslate_copper_ore"
            | "minecraft:deepslate_lapis_ore" => Some(Self::Ore {
                deepslate: true,
                tier: 1,
            }),
            "minecraft:deepslate_gold_ore"
            | "minecraft:deepslate_diamond_ore"
            | "minecraft:deepslate_emerald_ore"
            | "minecraft:deepslate_redstone_ore" => Some(Self::Ore {
                deepslate: true,
                tier: 2,
            }),
            "minecraft:crafting_table" => Some(Self::CraftingTable),
            "minecraft:chest" | "minecraft:trapped_chest" => Some(Self::Chest),
            "minecraft:bedrock"
            | "minecraft:moving_piston"
            | "minecraft:nether_portal"
            | "minecraft:end_portal"
            | "minecraft:end_portal_frame"
            | "minecraft:command_block"
            | "minecraft:barrier"
            | "minecraft:light"
            | "minecraft:end_gateway"
            | "minecraft:repeating_command_block"
            | "minecraft:chain_command_block"
            | "minecraft:structure_block"
            | "minecraft:jigsaw"
            | "minecraft:test_block"
            | "minecraft:test_instance_block" => Some(Self::Unbreakable),
            _ => native_core::button::Button::from_id(name)
                .map(|button| {
                    if button.arrow_sensitive() {
                        Self::WoodButton
                    } else {
                        Self::StoneButton
                    }
                })
                .or_else(|| {
                    native_core::wood::Wood::from_id(name)
                        .map(|_| Self::Wood)
                        .or_else(|| native_core::wood::Planks::from_id(name).map(|_| Self::Planks))
                }),
        }
    }

    pub fn hardness(self) -> f32 {
        match self {
            Self::Grass | Self::Gravel => 0.6,
            Self::Dirt | Self::Sand | Self::StoneButton | Self::WoodButton | Self::Lever => 0.5,
            Self::DirtPath => 0.65,
            Self::Snow => 0.1,
            Self::SnowBlock => 0.2,
            Self::HoneyBlock => 0.0,
            Self::RedstoneLamp => 0.3,
            Self::Banner => 1.0,
            Self::RedstoneBlock => 5.0,
            Self::Lodestone => 3.5,
            Self::Stone | Self::StoneBricks => 1.5,
            Self::Planks | Self::Cobblestone | Self::Wood => 2.0,
            Self::CraftingTable | Self::Chest => 2.5,
            Self::Stair {
                hardness_tenths, ..
            }
            | Self::Slab {
                hardness_tenths, ..
            } => f32::from(hardness_tenths) / 10.0,
            Self::Ore { deepslate, .. } => {
                if deepslate {
                    4.5
                } else {
                    3.0
                }
            }
            Self::Unbreakable => -1.0,
        }
    }

    pub fn needs_tool(self) -> bool {
        matches!(
            self,
            Self::Stone
                | Self::StoneBricks
                | Self::Cobblestone
                | Self::RedstoneBlock
                | Self::Lodestone
                | Self::Ore { .. }
                | Self::Snow
                | Self::SnowBlock
                | Self::Stair {
                    tool: StairTool::Pickaxe | StairTool::StonePickaxe,
                    ..
                }
                | Self::Slab {
                    tool: StairTool::Pickaxe | StairTool::StonePickaxe,
                    ..
                }
        )
    }
}

/// Vanilla's unmodified player destroy progress per tick, with no status effects.
pub fn basic_progress(material: Material, on_ground: bool, eyes_in_water: bool) -> f32 {
    if material == Material::Unbreakable {
        return 0.0;
    }
    let mut speed = 1.0_f32;
    if eyes_in_water {
        speed *= 0.2;
    }
    if !on_ground {
        speed /= 5.0;
    }
    speed / material.hardness() / if material.needs_tool() { 100.0 } else { 30.0 }
}

/// Mining properties for ordinary and custom tool components.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiningProperties {
    pub speed: f32,
    pub correct_for_drops: bool,
    pub damage_per_block: u32,
    pub can_destroy_in_creative: bool,
}

/// Player attributes and effects used by Java's block destroy calculation.
#[derive(Clone, Copy, Debug)]
pub struct MiningContext {
    pub on_ground: bool,
    pub eyes_in_water: bool,
    pub mining_efficiency: f64,
    pub block_break_speed: f64,
    pub submerged_mining_speed: f64,
    pub haste: Option<u8>,
    pub conduit_power: Option<u8>,
    pub mining_fatigue: Option<u8>,
}

impl Default for MiningContext {
    fn default() -> Self {
        Self {
            on_ground: true,
            eyes_in_water: false,
            mining_efficiency: 0.0,
            block_break_speed: 1.0,
            submerged_mining_speed: 0.2,
            haste: None,
            conduit_power: None,
            mining_fatigue: None,
        }
    }
}

pub fn mining_properties(material: Material, item: Option<&ItemStack>) -> Option<MiningProperties> {
    if item.is_some_and(|item| item.components().get("minecraft:tool").is_some()) {
        return None;
    }
    if basic_hand(item) {
        return Some(MiningProperties {
            speed: 1.0,
            correct_for_drops: !material.needs_tool(),
            damage_per_block: 0,
            can_destroy_in_creative: true,
        });
    }
    let item = item?;
    if item.count() != 1 || !ordinary_return_supported(item) {
        return None;
    }
    if item.id() == "minecraft:shears" {
        return Some(MiningProperties {
            speed: if matches!(
                material,
                Material::Stair {
                    tool: StairTool::Shears,
                    ..
                } | Material::Slab {
                    tool: StairTool::Shears,
                    ..
                }
            ) {
                5.0
            } else {
                1.0
            },
            correct_for_drops: !material.needs_tool(),
            damage_per_block: 1,
            can_destroy_in_creative: true,
        });
    }
    let (tier, kind) = item.id().strip_prefix("minecraft:")?.split_once('_')?;
    let speed = match tier {
        "wooden" => 2.0,
        "stone" => 4.0,
        "copper" => 5.0,
        "iron" => 6.0,
        "golden" => 12.0,
        "diamond" => 8.0,
        "netherite" => 9.0,
        _ => return None,
    };
    let effective = match kind {
        "shovel" => matches!(
            material,
            Material::Grass
                | Material::Dirt
                | Material::DirtPath
                | Material::Sand
                | Material::Gravel
                | Material::Snow
                | Material::SnowBlock
        ),
        "pickaxe" => matches!(
            material,
            Material::Stone
                | Material::StoneBricks
                | Material::Cobblestone
                | Material::StoneButton
                | Material::RedstoneBlock
                | Material::Lodestone
                | Material::Ore { .. }
                | Material::Stair {
                    tool: StairTool::Pickaxe | StairTool::StonePickaxe,
                    ..
                }
                | Material::Slab {
                    tool: StairTool::Pickaxe | StairTool::StonePickaxe,
                    ..
                }
        ),
        "axe" => matches!(
            material,
            Material::Planks
                | Material::CraftingTable
                | Material::Chest
                | Material::Banner
                | Material::Wood
                | Material::WoodButton
                | Material::Stair {
                    tool: StairTool::Axe,
                    ..
                }
                | Material::Slab {
                    tool: StairTool::Axe,
                    ..
                }
        ),
        "sword" | "hoe" => false,
        _ => return None,
    };
    Some(MiningProperties {
        speed: if effective { speed } else { 1.0 },
        correct_for_drops: !material.needs_tool()
            || (kind == "shovel"
                && matches!(material, Material::Snow | Material::SnowBlock)
                && effective)
            || (kind == "pickaxe"
                && effective
                && match material {
                    Material::Ore { tier: required, .. } => {
                        let harvest_tier = match tier {
                            "wooden" | "golden" => 0,
                            "stone" | "copper" => 1,
                            _ => 2,
                        };
                        harvest_tier >= required
                    }
                    Material::Stair {
                        tool: StairTool::StonePickaxe,
                        ..
                    }
                    | Material::Slab {
                        tool: StairTool::StonePickaxe,
                        ..
                    } => !matches!(tier, "wooden" | "golden"),
                    _ => true,
                }),
        damage_per_block: if kind == "sword" { 2 } else { 1 },
        can_destroy_in_creative: kind != "sword",
    })
}

/// Apply the default tool components to the active world's block tags.
pub fn mining_properties_with_tags(
    material: Material,
    item: Option<&ItemStack>,
    tags: u16,
) -> Option<MiningProperties> {
    use native_resources::mining_tags as t;
    if item.is_some_and(|item| item.components().get("minecraft:tool").is_some()) {
        return None;
    }
    if basic_hand(item) {
        return Some(MiningProperties {
            speed: 1.0,
            correct_for_drops: !material.needs_tool(),
            damage_per_block: 0,
            can_destroy_in_creative: true,
        });
    }
    let item = item?;
    if item.count() != 1 || !ordinary_return_supported(item) {
        return None;
    }
    if item.id() == "minecraft:shears" {
        let speed = if tags & t::SHEARS_EXTREME != 0 {
            15.0
        } else if tags & t::SHEARS_MAJOR != 0 {
            5.0
        } else if tags & t::SHEARS_MINOR != 0 {
            2.0
        } else {
            1.0
        };
        return Some(MiningProperties {
            speed,
            correct_for_drops: !material.needs_tool(),
            damage_per_block: 1,
            can_destroy_in_creative: true,
        });
    }
    let (tier, kind) = item.id().strip_prefix("minecraft:")?.split_once('_')?;
    let (speed, incorrect) = match tier {
        "wooden" => (2.0, t::WOODEN_INCORRECT),
        "stone" => (4.0, t::STONE_INCORRECT),
        "copper" => (5.0, t::COPPER_INCORRECT),
        "iron" => (6.0, t::IRON_INCORRECT),
        "golden" => (12.0, t::GOLD_INCORRECT),
        "diamond" => (8.0, t::DIAMOND_INCORRECT),
        "netherite" => (9.0, t::NETHERITE_INCORRECT),
        _ => return None,
    };
    if kind == "sword" {
        let speed = if tags & t::SWORD_INSTANT != 0 {
            f32::MAX
        } else if tags & t::SWORD_EFFICIENT != 0 {
            1.5
        } else {
            1.0
        };
        return Some(MiningProperties {
            speed,
            correct_for_drops: !material.needs_tool(),
            damage_per_block: 2,
            can_destroy_in_creative: false,
        });
    }
    let effective = match kind {
        "pickaxe" => t::PICKAXE,
        "axe" => t::AXE,
        "shovel" => t::SHOVEL,
        "hoe" => t::HOE,
        _ => return None,
    };
    Some(MiningProperties {
        speed: if tags & effective != 0 { speed } else { 1.0 },
        correct_for_drops: !material.needs_tool()
            || (tags & incorrect == 0 && tags & effective != 0),
        damage_per_block: 1,
        can_destroy_in_creative: true,
    })
}

/// Evaluate a saved custom tool component against the world's active block tags.
pub fn mining_properties_with_custom_tool(
    material: Material,
    item: &ItemStack,
    block: &str,
    tags: &native_resources::mining_tags::MiningTags,
) -> Option<MiningProperties> {
    if !ordinary_return_supported(item) {
        return None;
    }
    crate::custom_tool::Tool::parse(item.components().get("minecraft:tool")?)?.properties(
        material,
        block,
        Some(tags),
    )
}

/// Vanilla progress with the supported default tool component and no effects.
pub fn tool_progress(
    material: Material,
    item: Option<&ItemStack>,
    on_ground: bool,
    eyes_in_water: bool,
) -> Option<f32> {
    tool_progress_with_context(
        material,
        item,
        MiningContext {
            on_ground,
            eyes_in_water,
            ..Default::default()
        },
    )
}

pub fn tool_progress_with_context(
    material: Material,
    item: Option<&ItemStack>,
    context: MiningContext,
) -> Option<f32> {
    if material == Material::Unbreakable {
        return Some(0.0);
    }
    let properties = mining_properties(material, item)?;
    Some(progress_with_properties(material, properties, context))
}

fn progress_with_properties(
    material: Material,
    properties: MiningProperties,
    context: MiningContext,
) -> f32 {
    let speed = destroy_speed(properties.speed, context);
    speed
        / material.hardness()
        / if properties.correct_for_drops {
            30.0
        } else {
            100.0
        }
}

pub fn destroy_speed(base_speed: f32, context: MiningContext) -> f32 {
    let mut speed = base_speed;
    if speed > 1.0 {
        speed += context.mining_efficiency as f32;
    }
    if let Some(amplifier) = context.haste.into_iter().chain(context.conduit_power).max() {
        speed *= 1.0_f32 + (f32::from(amplifier) + 1.0) * 0.2_f32;
    }
    if let Some(amplifier) = context.mining_fatigue {
        speed *= 0.3_f64.powf(f64::from(amplifier) + 1.0) as f32;
    }
    speed *= context.block_break_speed as f32;
    if context.eyes_in_water {
        speed *= context.submerged_mining_speed as f32;
    }
    if !context.on_ground {
        speed /= 5.0;
    }
    speed
}

/// Items with unresolved components may have tool, enchantment or mining overrides.
pub(crate) fn basic_hand(item: Option<&ItemStack>) -> bool {
    item.is_none_or(|item| {
        ordinary_return_supported(item)
            && item.components().get("minecraft:tool").is_none()
            && (matches!(ordinary_item_default_max(item.id()), Some(16 | 64))
                || item.components().get("!minecraft:tool").is_some())
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BreakingProgress {
    /// Absolute block position, independent of a renderer's current chunk origin.
    pub position: IVec3,
    /// Vanilla's `destroy_stage_0` through `destroy_stage_9`.
    pub stage: u8,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    pub position: IVec3,
    /// `None` retains the experimental instant-break behavior for unsupported blocks.
    pub material: Option<Material>,
    pub tags: Option<u16>,
    /// A custom component must not fall back to the default item rules.
    pub custom: Option<Option<MiningProperties>>,
}

fn candidate_properties(
    candidate: Candidate,
    hand: Option<&ItemStack>,
) -> Option<MiningProperties> {
    if let Some(properties) = candidate.custom {
        return properties;
    }
    let material = candidate.material?;
    match candidate.tags {
        Some(tags) => mining_properties_with_tags(material, hand, tags),
        None => mining_properties(material, hand),
    }
}

#[derive(Clone, Debug)]
struct Active {
    candidate: Candidate,
    hand: Option<ItemStack>,
    progress: f32,
}

/// Client-side hold, target and cooldown state. Mutations occur on game ticks.
#[derive(Default)]
pub(crate) struct Mining {
    held: bool,
    started_this_tick: bool,
    cooldown: u8,
    active: Option<Active>,
}

impl Mining {
    pub fn held(&self) -> bool {
        self.held
    }

    pub fn hold_without_start(&mut self) {
        self.held = true;
    }

    pub fn is_destroying(&self) -> bool {
        self.active.is_some()
    }

    pub fn release(&mut self) {
        self.held = false;
        self.started_this_tick = false;
        self.active = None;
    }

    pub fn press(
        &mut self,
        target: Option<Candidate>,
        hand: Option<&ItemStack>,
        creative: bool,
    ) -> Option<IVec3> {
        self.press_with_context(target, hand, creative, MiningContext::default())
    }

    pub fn press_with_context(
        &mut self,
        target: Option<Candidate>,
        hand: Option<&ItemStack>,
        creative: bool,
        context: MiningContext,
    ) -> Option<IVec3> {
        self.held = true;
        let broken = self.begin(target, hand, creative, context);
        // A held, non-instant attack continues during this client tick.
        self.started_this_tick = broken.is_some();
        broken
    }

    fn begin(
        &mut self,
        target: Option<Candidate>,
        hand: Option<&ItemStack>,
        creative: bool,
        context: MiningContext,
    ) -> Option<IVec3> {
        self.active = None;
        let target = target?;
        // An unresolved component patch may change whether, how fast, or with
        // what effects a block can be broken. Do not assume instant breaking.
        if hand.is_some_and(|item| !ordinary_return_supported(item)) {
            return None;
        }
        if creative {
            let creative_allowed = hand
                .and_then(|item| item.components().get("minecraft:tool"))
                .map(|component| {
                    crate::custom_tool::Tool::parse(component).map(|tool| tool.creative())
                })
                .unwrap_or_else(|| {
                    Some(
                        mining_properties(Material::Dirt, hand)
                            .is_none_or(|properties| properties.can_destroy_in_creative),
                    )
                });
            if creative_allowed != Some(true) {
                return None;
            }
            self.cooldown = 5;
            return Some(target.position);
        }
        if target.material == Some(Material::Unbreakable) {
            return None;
        }
        if target.custom == Some(None) {
            return None;
        }
        if candidate_properties(target, hand).is_none() {
            // Preserve the existing experimental interaction until this block/item's
            // mining rules can be read from a complete registry and component model.
            self.cooldown = 5;
            return Some(target.position);
        }
        if target
            .material
            .zip(candidate_properties(target, hand))
            .map(|(material, properties)| progress_with_properties(material, properties, context))
            .is_some_and(|progress| progress >= 1.0)
        {
            self.cooldown = 5;
            return Some(target.position);
        }
        self.active = Some(Active {
            candidate: target,
            hand: hand.cloned(),
            progress: 0.0,
        });
        None
    }

    pub fn tick(
        &mut self,
        target: Option<Candidate>,
        hand: Option<&ItemStack>,
        creative: bool,
        on_ground: bool,
        eyes_in_water: bool,
    ) -> Option<IVec3> {
        self.tick_with_context(
            target,
            hand,
            creative,
            MiningContext {
                on_ground,
                eyes_in_water,
                ..Default::default()
            },
        )
    }

    pub fn tick_with_context(
        &mut self,
        target: Option<Candidate>,
        hand: Option<&ItemStack>,
        creative: bool,
        context: MiningContext,
    ) -> Option<IVec3> {
        if !self.held {
            return None;
        }
        if self.started_this_tick {
            self.started_this_tick = false;
            return None;
        }
        if target.is_none() {
            self.active = None;
            return None;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return None;
        }
        if creative {
            return self.begin(target, hand, true, context);
        }
        if target.is_some_and(|target| target.material == Some(Material::Unbreakable)) {
            self.active = None;
            return None;
        }
        let same = self.active.as_ref().is_some_and(|active| {
            target.is_some_and(|target| target.position == active.candidate.position)
                && match (&active.hand, hand) {
                    (None, None) => true,
                    (Some(old), Some(new)) => old.same_item_and_components(new),
                    _ => false,
                }
        });
        if !same {
            return self.begin(target, hand, false, context);
        }
        let target = target.expect("same target has a candidate");
        if target.material.is_none() {
            return self.begin(Some(target), hand, false, context);
        }
        let Some(properties) = candidate_properties(target, hand) else {
            self.active = None;
            return None;
        };
        let active = self.active.as_mut().expect("same target has active state");
        active.candidate = target;
        active.progress += progress_with_properties(
            target.material.expect("known material"),
            properties,
            context,
        );
        if active.progress >= 1.0 {
            let position = active.candidate.position;
            self.active = None;
            self.cooldown = 5;
            return Some(position);
        }
        None
    }

    pub fn progress(&self) -> Option<BreakingProgress> {
        self.active.as_ref().and_then(|active| {
            (active.progress > 0.0).then_some(BreakingProgress {
                position: active.candidate.position,
                stage: ((active.progress * 10.0) as u8).min(9),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::Tag;

    fn candidate(position: IVec3, material: Material) -> Candidate {
        Candidate {
            position,
            material: Some(material),
            tags: None,
            custom: None,
        }
    }

    #[test]
    fn mining_context_enables_instant_break_and_respects_fatigue() {
        let target = candidate(IVec3::ZERO, Material::Dirt);
        let fast = MiningContext {
            block_break_speed: 15.0,
            ..Default::default()
        };
        let mut mining = Mining::default();
        assert_eq!(
            mining.press_with_context(Some(target), None, false, fast),
            Some(IVec3::ZERO)
        );
        let fatigued = MiningContext {
            mining_fatigue: Some(0),
            ..fast
        };
        let mut mining = Mining::default();
        assert_eq!(
            mining.press_with_context(Some(target), None, false, fatigued),
            None
        );
        assert!(
            mining
                .tick_with_context(Some(target), None, false, fatigued)
                .is_none()
        );
        assert!(mining.progress().is_some());
    }

    #[test]
    fn release_and_target_or_item_change_reset_progress() {
        let stone = ItemStack::new("minecraft:stone".into(), 4, 64, Tag::Compound(vec![])).unwrap();
        let dirt = ItemStack::new("minecraft:dirt".into(), 4, 64, Tag::Compound(vec![])).unwrap();
        let first = candidate(IVec3::ZERO, Material::Dirt);
        let second = candidate(IVec3::X, Material::Dirt);
        let mut mining = Mining::default();
        assert!(mining.press(Some(first), Some(&stone), false).is_none());
        mining.tick(Some(first), Some(&stone), false, true, false);
        mining.tick(Some(first), Some(&stone), false, true, false);
        assert_eq!(mining.progress().unwrap().position, IVec3::ZERO);
        mining.tick(Some(second), Some(&stone), false, true, false);
        assert!(mining.progress().is_none());
        mining.tick(Some(second), Some(&stone), false, true, false);
        assert_eq!(mining.progress().unwrap().position, IVec3::X);
        mining.tick(Some(second), Some(&dirt), false, true, false);
        assert!(mining.progress().is_none());
        mining.tick(Some(second), Some(&dirt), false, true, false);
        assert!(mining.progress().is_some());
        mining.release();
        assert!(mining.progress().is_none());
        assert!(
            mining
                .tick(Some(second), Some(&dirt), false, true, false)
                .is_none()
        );
    }

    #[test]
    fn creative_click_and_hold_use_the_five_tick_delay() {
        let target = candidate(IVec3::ZERO, Material::Stone);
        let mut mining = Mining::default();
        assert_eq!(mining.press(Some(target), None, true), Some(IVec3::ZERO));
        assert!(mining.tick(Some(target), None, true, true, false).is_none());
        for _ in 0..8 {
            assert!(mining.tick(None, None, true, true, false).is_none());
        }
        for _ in 0..5 {
            assert!(mining.tick(Some(target), None, true, true, false).is_none());
        }
        assert_eq!(
            mining.tick(Some(target), None, true, true, false),
            Some(IVec3::ZERO)
        );
        mining.release();
        assert_eq!(mining.press(Some(target), None, true), Some(IVec3::ZERO));
    }

    #[test]
    fn bedrock_survives_held_mining_but_creative_can_break_it() {
        assert_eq!(
            Material::from_block_name("minecraft:bedrock"),
            Some(Material::Unbreakable)
        );
        let target = candidate(IVec3::ZERO, Material::Unbreakable);
        let mut mining = Mining::default();
        assert_eq!(mining.press(Some(target), None, false), None);
        for _ in 0..200 {
            assert_eq!(mining.tick(Some(target), None, false, true, false), None);
            assert!(mining.progress().is_none());
        }
        mining.release();
        assert_eq!(mining.press(Some(target), None, true), Some(IVec3::ZERO));
    }

    #[test]
    fn creative_sword_cannot_break_supported_or_unknown_blocks() {
        let sword =
            ItemStack::new("minecraft:copper_sword".into(), 1, 1, Tag::Compound(vec![])).unwrap();
        for material in [Some(Material::Stone), None] {
            let target = Candidate {
                position: IVec3::ZERO,
                material,
                tags: None,
                custom: None,
            };
            let mut mining = Mining::default();
            assert!(mining.press(Some(target), Some(&sword), true).is_none());
            for _ in 0..10 {
                assert!(
                    mining
                        .tick(Some(target), Some(&sword), true, true, false)
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn unresolved_tool_component_never_breaks_blocks_speculatively() {
        let item = ItemStack::new(
            "minecraft:diamond_pickaxe".into(),
            1,
            1,
            Tag::Compound(vec![("minecraft:tool".into(), Tag::Compound(vec![]))]),
        )
        .unwrap();
        for material in [Some(Material::Stone), None] {
            for creative in [false, true] {
                let target = Candidate {
                    position: IVec3::ZERO,
                    material,
                    tags: None,
                    custom: None,
                };
                let mut mining = Mining::default();
                assert_eq!(mining.press(Some(target), Some(&item), creative), None);
                for _ in 0..20 {
                    assert_eq!(
                        mining.tick(Some(target), Some(&item), creative, true, false),
                        None
                    );
                    assert!(mining.progress().is_none());
                }
            }
        }
    }

    #[test]
    fn removed_tool_component_uses_hand_rules_including_creative() {
        let item = ItemStack::new(
            "minecraft:diamond_sword".into(),
            1,
            1,
            Tag::Compound(vec![("!minecraft:tool".into(), Tag::Compound(vec![]))]),
        )
        .unwrap();
        for tags in [None, Some(0), Some(u16::MAX)] {
            let target = Candidate {
                position: IVec3::ZERO,
                material: Some(Material::Stone),
                tags,
                custom: None,
            };
            let properties = candidate_properties(target, Some(&item)).unwrap();
            assert_eq!(properties.speed, 1.0);
            assert!(!properties.correct_for_drops);
            assert_eq!(properties.damage_per_block, 0);
            assert!(properties.can_destroy_in_creative);
            let mut mining = Mining::default();
            assert_eq!(
                mining.press(Some(target), Some(&item), true),
                Some(IVec3::ZERO)
            );
        }
    }

    #[test]
    fn custom_tool_creative_permission_and_unresolved_rules_are_respected() {
        use native_persistence::nbt::TagType;
        for (allowed, expected) in [(false, None), (true, Some(IVec3::ZERO))] {
            let component = Tag::Compound(vec![
                (
                    "rules".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ),
                (
                    "can_destroy_blocks_in_creative".into(),
                    Tag::Byte(i8::from(allowed)),
                ),
            ]);
            let item = ItemStack::new(
                "minecraft:diamond_pickaxe".into(),
                1,
                1,
                Tag::Compound(vec![("minecraft:tool".into(), component)]),
            )
            .unwrap();
            let target = Candidate {
                position: IVec3::ZERO,
                material: Some(Material::Stone),
                tags: None,
                custom: Some(None),
            };
            let mut creative = Mining::default();
            assert_eq!(creative.press(Some(target), Some(&item), true), expected);
            let mut survival = Mining::default();
            assert_eq!(survival.press(Some(target), Some(&item), false), None);
            for _ in 0..20 {
                assert_eq!(
                    survival.tick(Some(target), Some(&item), false, true, false),
                    None
                );
            }
        }
    }

    #[test]
    fn changing_the_block_at_the_same_position_preserves_accumulated_progress() {
        let dirt = candidate(IVec3::ZERO, Material::Dirt);
        let stone = candidate(IVec3::ZERO, Material::Stone);
        let mut mining = Mining::default();
        mining.press(Some(dirt), None, false);
        mining.tick(Some(dirt), None, false, true, false);
        for _ in 0..5 {
            mining.tick(Some(dirt), None, false, true, false);
        }
        let stage = mining.progress().unwrap().stage;
        mining.tick(Some(stone), None, false, true, false);
        assert_eq!(mining.progress().unwrap().stage, stage);
    }
}
