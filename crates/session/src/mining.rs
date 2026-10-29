//! Basic block-breaking progress for supported ordinary items and blocks.

use glam::IVec3;

use crate::inventory::{ItemStack, ordinary_item_default_max, ordinary_return_supported};

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
pub enum Material {
    Grass,
    Dirt,
    Sand,
    Gravel,
    DirtPath,
    Stone,
    StoneBricks,
    StoneButton,
    Cobblestone,
    Wood,
    Planks,
    CraftingTable,
    Unbreakable,
}

impl Material {
    pub fn from_block_name(name: &str) -> Option<Self> {
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
            "minecraft:stone" => Some(Self::Stone),
            "minecraft:stone_bricks" => Some(Self::StoneBricks),
            "minecraft:stone_button" => Some(Self::StoneButton),
            "minecraft:cobblestone" => Some(Self::Cobblestone),
            "minecraft:crafting_table" => Some(Self::CraftingTable),
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
            _ => native_core::wood::Wood::from_id(name)
                .map(|_| Self::Wood)
                .or_else(|| native_core::wood::Planks::from_id(name).map(|_| Self::Planks)),
        }
    }

    pub fn hardness(self) -> f32 {
        match self {
            Self::Grass | Self::Gravel => 0.6,
            Self::Dirt | Self::Sand | Self::StoneButton => 0.5,
            Self::DirtPath => 0.65,
            Self::Stone | Self::StoneBricks => 1.5,
            Self::Planks | Self::Cobblestone | Self::Wood => 2.0,
            Self::CraftingTable => 2.5,
            Self::Unbreakable => -1.0,
        }
    }

    pub fn needs_tool(self) -> bool {
        matches!(self, Self::Stone | Self::StoneBricks | Self::Cobblestone)
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

/// Mining properties for ordinary, unenchanted item prototypes and basic blocks.
/// Unknown component patches and custom tools are deliberately left unresolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiningProperties {
    pub speed: f32,
    pub correct_for_drops: bool,
    pub damage_per_block: u32,
    pub can_destroy_in_creative: bool,
}

pub fn mining_properties(material: Material, item: Option<&ItemStack>) -> Option<MiningProperties> {
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
        ),
        "pickaxe" => matches!(
            material,
            Material::Stone | Material::StoneBricks | Material::Cobblestone | Material::StoneButton
        ),
        "axe" => matches!(
            material,
            Material::Planks | Material::CraftingTable | Material::Wood
        ),
        "sword" | "hoe" => false,
        _ => return None,
    };
    Some(MiningProperties {
        speed: if effective { speed } else { 1.0 },
        correct_for_drops: !material.needs_tool() || (kind == "pickaxe" && effective),
        damage_per_block: if kind == "sword" { 2 } else { 1 },
        can_destroy_in_creative: kind != "sword",
    })
}

/// Vanilla progress with the supported default tool component and no effects.
pub fn tool_progress(
    material: Material,
    item: Option<&ItemStack>,
    on_ground: bool,
    eyes_in_water: bool,
) -> Option<f32> {
    if material == Material::Unbreakable {
        return Some(0.0);
    }
    let properties = mining_properties(material, item)?;
    let mut speed = properties.speed;
    if eyes_in_water {
        speed *= 0.2;
    }
    if !on_ground {
        speed /= 5.0;
    }
    Some(
        speed
            / material.hardness()
            / if properties.correct_for_drops {
                30.0
            } else {
                100.0
            },
    )
}

/// Items with unresolved components may have tool, enchantment or mining overrides.
pub(crate) fn basic_hand(item: Option<&ItemStack>) -> bool {
    item.is_none_or(|item| {
        ordinary_item_default_max(item.id()) == Some(64) && ordinary_return_supported(item)
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
        self.held = true;
        let broken = self.begin(target, hand, creative);
        // A held, non-instant attack continues during this client tick.
        self.started_this_tick = broken.is_some();
        broken
    }

    fn begin(
        &mut self,
        target: Option<Candidate>,
        hand: Option<&ItemStack>,
        creative: bool,
    ) -> Option<IVec3> {
        self.active = None;
        let target = target?;
        if creative {
            if mining_properties(Material::Dirt, hand)
                .is_some_and(|properties| !properties.can_destroy_in_creative)
            {
                return None;
            }
            self.cooldown = 5;
            return Some(target.position);
        }
        if target.material == Some(Material::Unbreakable) {
            return None;
        }
        if target
            .material
            .is_none_or(|material| mining_properties(material, hand).is_none())
        {
            // Preserve the existing experimental interaction until this block/item's
            // mining rules can be read from a complete registry and component model.
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
            return self.begin(target, hand, true);
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
            return self.begin(target, hand, false);
        }
        let target = target.expect("same target has a candidate");
        if target.material.is_none() {
            return self.begin(Some(target), hand, false);
        }
        let active = self.active.as_mut().expect("same target has active state");
        active.candidate = target;
        active.progress += tool_progress(
            target.material.expect("known material"),
            hand,
            on_ground,
            eyes_in_water,
        )
        .expect("active tool has supported mining properties");
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
        }
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
