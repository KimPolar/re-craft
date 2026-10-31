//! Item stacks and the ordinary container pickup operation.
//!
//! Item-specific click overrides (for example bundles) and recipe result slots
//! require their own handlers; callers must not treat this as those handlers.

use std::{collections::BTreeSet, sync::Arc};

use glam::IVec3;
use native_core::random::LegacyRandom;
use native_persistence::nbt::{JavaString, Tag};
use native_resources::{
    enchantments::{EquipmentSlot, OxygenEnchantments},
    packs::ResourceId,
    recipes::{CraftingRecipe, CraftingRecipes, RecipeBookInventory},
};

#[derive(Clone, Debug, PartialEq)]
pub struct ItemStack {
    id: String,
    count: u32,
    max_count: Option<u32>,
    components: Tag,
}

impl ItemStack {
    /// `components` is the effective component map, not just its saved patch.
    /// Callers must resolve the item's prototype before creating the stack.
    pub fn new(id: String, count: u32, max_count: u32, components: Tag) -> Option<Self> {
        if id.is_empty()
            || count == 0
            || count > i32::MAX as u32
            || !(1..=99).contains(&max_count)
            || !matches!(components, Tag::Compound(_))
        {
            return None;
        }
        Some(Self {
            id,
            count,
            max_count: Some(max_count),
            components,
        })
    }

    /// Retain a saved component patch until the item's prototype is available.
    /// An unresolved stack cannot participate in pickup or placement.
    pub fn unresolved(id: String, count: u32, components: Tag) -> Option<Self> {
        if id.is_empty() || !(1..=99).contains(&count) || !matches!(components, Tag::Compound(_)) {
            return None;
        }
        Some(Self {
            id,
            count,
            max_count: None,
            components,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn count(&self) -> u32 {
        self.count
    }
    pub(crate) fn with_count(&self, count: u32) -> Option<Self> {
        if count == 0 || count > i32::MAX as u32 {
            return None;
        }
        let mut stack = self.clone();
        stack.count = count;
        Some(stack)
    }
    pub fn max_count(&self) -> Option<u32> {
        self.max_count
    }
    pub fn components(&self) -> &Tag {
        &self.components
    }

    /// Whether the item displays the Java 26.3 enchantment glint. Saved stacks
    /// may still contain a component patch rather than the complete prototype.
    pub fn has_glint(&self) -> bool {
        if let Some(Tag::Byte(value)) = self.components.get("minecraft:enchantment_glint_override")
        {
            return *value != 0;
        }
        if self
            .components
            .get("!minecraft:enchantment_glint_override")
            .is_none()
            && matches!(
                self.id.as_str(),
                "minecraft:enchanted_golden_apple"
                    | "minecraft:experience_bottle"
                    | "minecraft:written_book"
                    | "minecraft:nether_star"
                    | "minecraft:enchanted_book"
                    | "minecraft:end_crystal"
                    | "minecraft:debug_stick"
            )
        {
            return true;
        }
        if self.id == "minecraft:compass"
            && self.components.get("minecraft:lodestone_tracker").is_some()
            && self
                .components
                .get("!minecraft:lodestone_tracker")
                .is_none()
        {
            return true;
        }
        matches!(
            self.components.get("minecraft:enchantments"),
            Some(Tag::Compound(enchantments)) if !enchantments.is_empty()
        )
    }

    /// Vanilla's default durability bar for the supported ordinary tools.
    pub fn durability_bar(&self) -> Option<(u32, [u8; 3])> {
        if self.max_count != Some(1) {
            return None;
        }
        let maximum = ordinary_item_default_durability(&self.id)
            .or_else(|| crate::equippable::vanilla_equipment(&self.id).and_then(|item| item.2))?;
        let damage = self
            .components
            .get("minecraft:damage")?
            .as_java_int()?
            .clamp(0, maximum as i32);
        if damage == 0 {
            return None;
        }
        let width = (13.0_f32 - damage as f32 * 13.0 / maximum as f32)
            .round()
            .clamp(0.0, 13.0) as u32;
        let fraction = ((maximum as f32 - damage as f32) / maximum as f32).max(0.0);
        let hue = fraction / 3.0 * 6.0;
        let sector = hue as u32;
        let f = hue - sector as f32;
        let color = match sector {
            0 => [255, ((1.0 - (1.0 - f)) * 255.0) as u8, 0],
            1 => [((1.0 - f) * 255.0) as u8, 255, 0],
            _ => [0, 255, 0],
        };
        Some((width, color))
    }

    pub fn same_item_and_components(&self, other: &Self) -> bool {
        self.max_count.is_some()
            && self.id == other.id
            && self.max_count == other.max_count
            && (equal_tag(&self.components, &other.components)
                || (max_component_consistent(self)
                    && max_component_consistent(other)
                    && matches!((&self.components, &other.components), (Tag::Compound(a), Tag::Compound(b)) if equal_compounds(a, b, true))))
    }
}

fn repair_damage(item: &ItemStack) -> anyhow::Result<Option<i32>> {
    use anyhow::{Context, ensure};
    ensure!(item.max_count.is_some(), "unresolved equipped item");
    if item.components.get("minecraft:unbreakable").is_some()
        || item.components.get("!minecraft:max_damage").is_some()
    {
        return Ok(None);
    }
    let damage = match item.components.get("minecraft:damage") {
        None | Some(Tag::Int(0)) => return Ok(None),
        Some(Tag::Int(value)) if *value > 0 => *value,
        _ => anyhow::bail!("invalid item damage"),
    };
    let max = match item.components.get("minecraft:max_damage") {
        Some(Tag::Int(value)) if *value > 0 => *value,
        Some(_) => anyhow::bail!("invalid maximum durability"),
        None => ordinary_item_default_durability(item.id())
            .or_else(|| crate::equippable::vanilla_equipment(item.id()).and_then(|item| item.2))
            .and_then(|value| i32::try_from(value).ok())
            .context("unknown item durability")?,
    };
    Ok(Some(damage.min(max)))
}

pub(crate) fn item_enchantments(item: &ItemStack) -> anyhow::Result<Vec<(ResourceId, u32)>> {
    use anyhow::{Context, ensure};
    let Some(enchantments) = item.components.get("minecraft:enchantments") else {
        return Ok(Vec::new());
    };
    let Tag::Compound(entries) = enchantments else {
        anyhow::bail!("item enchantments are not a compound")
    };
    let mut ids = BTreeSet::new();
    let mut result = Vec::with_capacity(entries.len());
    for (name, level) in entries {
        let id =
            ResourceId::parse(&String::from_utf16(&name.0).context("invalid enchantment ID")?)?;
        ensure!(ids.insert(id.clone()), "duplicate enchantment");
        let level = level
            .as_java_int()
            .filter(|level| (1..=255).contains(level))
            .context("invalid enchantment level")? as u32;
        result.push((id, level));
    }
    Ok(result)
}

fn quick_craft_amount(carried: &ItemStack, slots: usize, button: ClickButton) -> u32 {
    match button {
        ClickButton::Primary => ((carried.count as f32 / slots as f32).floor()) as u32,
        ClickButton::Secondary => 1,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickButton {
    Primary,
    Secondary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryPointerTarget {
    Slot(PlayerSlot),
    Outside,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryPointerEvent {
    Press {
        target: InventoryPointerTarget,
        button: ClickButton,
        shift: bool,
    },
    Move {
        slot: PlayerSlot,
    },
    Release {
        target: InventoryPointerTarget,
        button: ClickButton,
        shift: bool,
    },
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerSlot {
    /// Hotbar 0–8, followed by the three main inventory rows 9–35.
    Storage(u8),
    /// The active 2×2 or 3×3 crafting grid in reading order.
    Crafting(u8),
    CraftResult,
    Armor(u8),
    Offhand,
    /// Slot in the open single or double chest.
    Chest(u8),
}

fn armor_slot(index: u8) -> Option<EquipmentSlot> {
    Some(match index {
        0 => EquipmentSlot::Head,
        1 => EquipmentSlot::Chest,
        2 => EquipmentSlot::Legs,
        3 => EquipmentSlot::Feet,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryMenu {
    Player,
    /// Absolute block position, retained across loaded-world origin shifts.
    CraftingTable {
        position: IVec3,
    },
    Chest {
        position: IVec3,
        other: Option<IVec3>,
    },
}

#[derive(Clone, Debug, PartialEq)]
struct ChestInventory {
    position: IVec3,
    other: Option<IVec3>,
    slots: Vec<Option<ItemStack>>,
    title: Option<String>,
    title_component: Option<Arc<Tag>>,
}

/// The player's saved recipe-book state; unknown recipe IDs remain preserved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecipeBookState {
    pub known: BTreeSet<ResourceId>,
    pub highlighted: BTreeSet<ResourceId>,
    pub open: bool,
    pub filtering: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecipeBookPlacement {
    Unchanged,
    Placed,
    Ghost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerMode {
    Tracking,
    ReleaseOnly,
    SuppressRelease,
}

#[derive(Clone, Debug, PartialEq)]
struct PointerGesture {
    button: ClickButton,
    mode: PointerMode,
    slots: Vec<PlayerSlot>,
}

pub struct QuickCraftSlotPreview {
    pub slot: PlayerSlot,
    /// A single selected slot is hidden while dragging, like the Java screen.
    pub item: Option<ItemStack>,
    pub capped: bool,
}

pub struct QuickCraftPreview {
    pub slots: Vec<QuickCraftSlotPreview>,
    pub carried: Option<ItemStack>,
    pub exhausted: bool,
}

pub(crate) enum PointerRelease {
    None,
    Click,
    Slot(PlayerSlot),
    Distributed { crafting: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerInventory {
    storage: [Option<ItemStack>; 36],
    crafting: [Option<ItemStack>; 9],
    craft_result: Option<ItemStack>,
    armor: [Option<ItemStack>; 4],
    offhand: Option<ItemStack>,
    chest: Option<ChestInventory>,
    cursor: Option<ItemStack>,
    selected: u8,
    menu: Option<InventoryMenu>,
    recipe_book: Arc<RecipeBookState>,
    ghost_recipe: Option<ResourceId>,
    pointer: Option<PointerGesture>,
    revision: u64,
    enchantments: Option<Arc<OxygenEnchantments>>,
}

/// Inventory stacks scatter on death; closing a container may separately
/// return its inputs or throw overflow along the player's view direction.
pub(crate) struct DeathDrops {
    pub inventory: Vec<ItemStack>,
    pub container_overflow: Vec<ItemStack>,
    pub retained_without_keep: bool,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self {
            storage: std::array::from_fn(|_| None),
            crafting: std::array::from_fn(|_| None),
            craft_result: None,
            armor: std::array::from_fn(|_| None),
            offhand: None,
            chest: None,
            cursor: None,
            selected: 0,
            menu: None,
            recipe_book: Arc::new(RecipeBookState::default()),
            ghost_recipe: None,
            pointer: None,
            revision: 0,
            enchantments: None,
        }
    }
}

impl PlayerInventory {
    /// Produce a transactional repair result; no replacement means no item
    /// changed. The caller stages the RNG until the whole orb pickup succeeds.
    pub(crate) fn repair_with_experience(
        &self,
        mut amount: i32,
        random: &mut LegacyRandom,
    ) -> anyhow::Result<(i32, Option<Self>)> {
        use anyhow::{Context, ensure};
        ensure!(amount > 0, "experience orb value must be positive");
        let mut candidates = self.experience_repair_candidates()?;
        if candidates.is_empty() {
            return Ok((amount, None));
        }
        let mut next = self.clone();
        let mut changed = false;
        for _ in 0..128 {
            if candidates.is_empty() {
                return Ok((amount, changed.then_some(next)));
            }
            let index = random
                .next_int_bound(i32::try_from(candidates.len())?)
                .context("no repair candidate")? as usize;
            let slot = candidates[index];
            let registry = next
                .enchantments
                .as_ref()
                .cloned()
                .context("active enchantment registry is unavailable")?;
            let item = match slot {
                PlayerSlot::Storage(index) => next.storage.get_mut(index as usize),
                PlayerSlot::Offhand => Some(&mut next.offhand),
                PlayerSlot::Armor(index) => next.armor.get_mut(index as usize),
                _ => None,
            }
            .and_then(Option::as_mut)
            .context("repair candidate disappeared")?;
            let damage = repair_damage(item)?.context("repair candidate has no damage")?;
            let mut repair = amount as f32;
            let mut has_repair_effect = false;
            for (id, level) in item_enchantments(item)? {
                // Java applies every repair effect on the chosen item.
                if let Some(next_value) = registry.apply_repair_with_xp(&id, level, repair)? {
                    ensure!(
                        !has_repair_effect,
                        "multiple enchantment repair-effect order is not supported"
                    );
                    has_repair_effect = true;
                    repair = next_value;
                }
            }
            let possible = (repair as i32).max(0);
            let restored = possible.min(damage);
            let Tag::Compound(components) = &mut item.components else {
                anyhow::bail!("item components are invalid")
            };
            let (_, field) = components
                .iter_mut()
                .find(|(name, _)| name.matches("minecraft:damage"))
                .context("repair damage component is missing")?;
            let new_damage = damage - restored;
            if *field != Tag::Int(new_damage) {
                *field = Tag::Int(new_damage);
                next.revision = next.revision.wrapping_add(1);
                changed = true;
            }
            if restored <= 0 {
                return Ok((0, changed.then_some(next)));
            }
            amount = amount.wrapping_sub(restored.wrapping_mul(amount) / possible);
            if amount <= 0 {
                return Ok((0, changed.then_some(next)));
            }
            candidates = next.experience_repair_candidates()?;
        }
        anyhow::bail!("repair-with-XP effects did not converge")
    }

    fn experience_repair_candidates(&self) -> anyhow::Result<Vec<PlayerSlot>> {
        use anyhow::Context;
        let slots = [
            (PlayerSlot::Storage(self.selected), EquipmentSlot::Mainhand),
            (PlayerSlot::Offhand, EquipmentSlot::Offhand),
            (PlayerSlot::Armor(3), EquipmentSlot::Feet),
            (PlayerSlot::Armor(2), EquipmentSlot::Legs),
            (PlayerSlot::Armor(1), EquipmentSlot::Chest),
            (PlayerSlot::Armor(0), EquipmentSlot::Head),
        ];
        let mut candidates = Vec::new();
        for (slot, equipped) in slots {
            let Some(item) = self.slot(slot) else {
                continue;
            };
            anyhow::ensure!(item.max_count.is_some(), "unresolved equipped item");
            let enchantments = item_enchantments(item)?;
            if enchantments.is_empty() {
                continue;
            }
            if repair_damage(item)?.is_none() {
                continue;
            }
            let registry = self
                .enchantments
                .as_ref()
                .context("active enchantment registry is unavailable")?;
            for (id, _) in enchantments {
                if registry.repairs_with_xp(&id, equipped)? {
                    // Java adds one entry per matching enchantment.
                    candidates.push(slot);
                }
            }
        }
        Ok(candidates)
    }
    /// Java creates a fresh inventory on death, copying the old slots only
    /// for keep-inventory or spectator players. Recipe-book data is separate.
    pub(crate) fn reset_after_death(&mut self, keep_inventory: bool) {
        if !keep_inventory {
            self.storage.fill(None);
            self.armor.fill(None);
            self.offhand = None;
            self.selected = 0;
        }
        self.crafting.fill(None);
        self.craft_result = None;
        self.cursor = None;
        self.menu = None;
        self.ghost_recipe = None;
        self.pointer = None;
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn set_enchantments(&mut self, enchantments: Arc<OxygenEnchantments>) {
        self.enchantments = Some(enchantments);
    }

    fn can_take_equipment(&self, item: Option<&ItemStack>, creative: bool) -> bool {
        let Some(item) = item else { return true };
        if creative {
            return true;
        }
        let Some(enchantments) = item.components.get("minecraft:enchantments") else {
            return true;
        };
        let Tag::Compound(entries) = enchantments else {
            return false;
        };
        entries.iter().all(|(name, level)| {
            let Some(registry) = &self.enchantments else {
                return false;
            };
            let Ok(name) = String::from_utf16(&name.0) else {
                return false;
            };
            let Ok(id) = ResourceId::parse(&name) else {
                return false;
            };
            level
                .as_java_int()
                .is_some_and(|value| (1..=255).contains(&value))
                && registry
                    .prevents_armor_change(&id)
                    .is_ok_and(|prevent| !prevent)
        })
    }

    fn armor_can_accept(&self, index: u8, item: &ItemStack) -> bool {
        armor_slot(index).is_some_and(|slot| crate::equippable::player_can_equip(item, slot))
    }

    /// Temporary demo inventory. The saved-world path must load actual player data.
    pub fn demo() -> Self {
        let mut inventory = Self::default();
        for (index, id) in ["minecraft:grass_block", "minecraft:dirt", "minecraft:stone"]
            .into_iter()
            .enumerate()
        {
            inventory.storage[index] = ItemStack::new(id.into(), 64, 64, Tag::Compound(vec![]));
        }
        inventory
    }

    pub fn slot(&self, slot: PlayerSlot) -> Option<&ItemStack> {
        match slot {
            PlayerSlot::Storage(index) => self.storage.get(index as usize)?.as_ref(),
            PlayerSlot::Crafting(index) if (index as usize) < self.crafting_len() => {
                self.crafting.get(index as usize)?.as_ref()
            }
            PlayerSlot::Crafting(_) => None,
            PlayerSlot::CraftResult => self.craft_result.as_ref(),
            PlayerSlot::Armor(index) => self.armor.get(index as usize)?.as_ref(),
            PlayerSlot::Offhand => self.offhand.as_ref(),
            PlayerSlot::Chest(index) if matches!(self.menu, Some(InventoryMenu::Chest { .. })) => {
                self.chest.as_ref()?.slots.get(index as usize)?.as_ref()
            }
            PlayerSlot::Chest(_) => None,
        }
    }

    pub fn cursor(&self) -> Option<&ItemStack> {
        self.cursor.as_ref()
    }

    /// Temporary container state cannot be represented in a saved player inventory.
    pub fn ready_for_player_save(&self) -> bool {
        self.menu.is_none()
            && self.cursor.is_none()
            && self.crafting.iter().all(Option::is_none)
            && self.craft_result.is_none()
            && self.pointer.is_none()
            && self.chest.is_none()
    }
    pub fn selected(&self) -> u8 {
        self.selected
    }
    pub fn is_open(&self) -> bool {
        self.menu.is_some()
    }
    pub fn menu(&self) -> Option<InventoryMenu> {
        self.menu
    }
    pub fn recipe_book(&self) -> &RecipeBookState {
        &self.recipe_book
    }
    pub fn set_recipe_book(&mut self, book: RecipeBookState) {
        if *self.recipe_book != book {
            self.recipe_book = Arc::new(book);
            self.revision = self.revision.wrapping_add(1);
        }
    }
    pub fn set_recipe_book_settings(&mut self, open: bool, filtering: bool) {
        if self.recipe_book.open != open || self.recipe_book.filtering != filtering {
            let book = Arc::make_mut(&mut self.recipe_book);
            book.open = open;
            book.filtering = filtering;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Determine supported inventory-change advancement rewards for changed slots.
    pub fn changed_recipe_unlocks(
        &self,
        before: &Self,
        recipes: &CraftingRecipes,
    ) -> BTreeSet<ResourceId> {
        let changed = self
            .storage
            .iter()
            .zip(&before.storage)
            .chain(std::iter::once((&self.offhand, &before.offhand)))
            .filter_map(|(after, previous)| {
                (after != previous).then_some(after.as_ref()).flatten()
            });
        changed
            .flat_map(|item| recipes.unlocked_by_item(item.id()).iter())
            .filter(|id| !self.recipe_book.known.contains(*id))
            .cloned()
            .collect()
    }

    pub fn unlock_recipes(&mut self, additions: BTreeSet<ResourceId>) {
        let mut updated = false;
        let book = Arc::make_mut(&mut self.recipe_book);
        for id in additions {
            if book.known.insert(id.clone()) {
                book.highlighted.insert(id);
                updated = true;
            }
        }
        if updated {
            self.revision = self.revision.wrapping_add(1);
        }
    }
    pub fn ghost_recipe(&self) -> Option<&ResourceId> {
        self.ghost_recipe.as_ref()
    }
    pub fn clear_ghost_recipe(&mut self) {
        if self.ghost_recipe.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Java counts usable non-equipment inventory and crafting stacks, with
    /// each stack capped at its own maximum when checking recipe availability.
    pub fn recipe_book_available(&self) -> Vec<(&str, u32)> {
        self.storage
            .iter()
            .chain(&self.crafting[..self.crafting_len()])
            .flatten()
            .filter(|item| recipe_book_usable(item))
            .map(|item| {
                (
                    item.id(),
                    item.count.min(item.max_count.expect("usable stack")),
                )
            })
            .collect()
    }

    /// Begin a pointer gesture. An empty cursor performs the normal click at
    /// press time; a carried stack waits until release or a quick-craft drag.
    pub(crate) fn pointer_press(
        &mut self,
        target: InventoryPointerTarget,
        button: ClickButton,
    ) -> bool {
        if !self.is_open() {
            return false;
        }
        let immediate = self.cursor.is_none() && target != InventoryPointerTarget::Background;
        let mode = if immediate {
            PointerMode::SuppressRelease
        } else if target == InventoryPointerTarget::Background {
            PointerMode::ReleaseOnly
        } else {
            PointerMode::Tracking
        };
        self.pointer = Some(PointerGesture {
            button,
            mode,
            slots: Vec::new(),
        });
        self.revision = self.revision.wrapping_add(1);
        immediate
    }

    fn quick_slot_compatible(&self, slot: PlayerSlot, carried: &ItemStack) -> bool {
        let valid = match slot {
            PlayerSlot::Storage(index) => index < 36,
            PlayerSlot::Crafting(index) => usize::from(index) < self.crafting_len(),
            PlayerSlot::Offhand => matches!(self.menu, Some(InventoryMenu::Player)),
            PlayerSlot::Armor(index) => {
                matches!(self.menu, Some(InventoryMenu::Player))
                    && self.armor_can_accept(index, carried)
            }
            PlayerSlot::CraftResult => false,
            PlayerSlot::Chest(index) => {
                matches!(self.menu, Some(InventoryMenu::Chest { .. }))
                    && self
                        .chest
                        .as_ref()
                        .is_some_and(|chest| usize::from(index) < chest.slots.len())
            }
        };
        if !valid || !basic_pickup_supported(carried) {
            return false;
        }
        let Some(max) = carried.max_count else {
            return false;
        };
        self.slot(slot).is_none_or(|item| {
            basic_pickup_supported(item)
                && item.same_item_and_components(carried)
                && item.count <= max
        })
    }

    pub(crate) fn pointer_visit(&mut self, slot: PlayerSlot) {
        let Some(pointer) = &self.pointer else { return };
        let Some(carried) = &self.cursor else { return };
        if pointer.mode != PointerMode::Tracking
            || pointer.slots.contains(&slot)
            || pointer.slots.len() >= carried.count as usize
            || !self.quick_slot_compatible(slot, carried)
        {
            return;
        }
        self.pointer
            .as_mut()
            .expect("checked pointer")
            .slots
            .push(slot);
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn pointer_cancel(&mut self) {
        if self.pointer.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(crate) fn pointer_release(&mut self, button: ClickButton) -> PointerRelease {
        let Some(pointer) = self.pointer.take() else {
            return PointerRelease::None;
        };
        self.revision = self.revision.wrapping_add(1);
        if pointer.button != button || pointer.mode == PointerMode::SuppressRelease {
            return PointerRelease::None;
        }
        if pointer.mode == PointerMode::ReleaseOnly || pointer.slots.is_empty() {
            return PointerRelease::Click;
        }
        if pointer.slots.len() == 1 {
            return PointerRelease::Slot(pointer.slots[0]);
        }
        let Some(carried) = self.cursor.clone() else {
            return PointerRelease::None;
        };
        if !basic_pickup_supported(&carried) || pointer.slots.len() > carried.count as usize {
            return PointerRelease::None;
        }
        let per_slot = quick_craft_amount(&carried, pointer.slots.len(), button);
        let mut remaining = carried.count;
        let mut crafting = false;
        for slot in pointer.slots {
            crafting |= matches!(slot, PlayerSlot::Crafting(_));
            if !self.quick_slot_compatible(slot, &carried) {
                continue;
            }
            let before = self.slot(slot).map_or(0, ItemStack::count);
            let limit = if matches!(slot, PlayerSlot::Armor(_)) {
                1
            } else {
                carried.max_count.unwrap()
            };
            let after = before.saturating_add(per_slot).min(limit);
            remaining -= after - before;
            let mut item = carried.clone();
            item.count = after;
            *self.mutable_slot(slot).expect("compatible slot") = Some(item);
        }
        self.cursor = if remaining == 0 {
            None
        } else {
            let mut item = carried;
            item.count = remaining;
            Some(item)
        };
        PointerRelease::Distributed { crafting }
    }

    pub fn quick_craft_preview(&self) -> Option<QuickCraftPreview> {
        let pointer = self.pointer.as_ref()?;
        let carried = self.cursor.as_ref()?;
        if pointer.mode != PointerMode::Tracking || pointer.slots.is_empty() {
            return None;
        }
        if pointer.slots.len() == 1 {
            return Some(QuickCraftPreview {
                slots: vec![QuickCraftSlotPreview {
                    slot: pointer.slots[0],
                    item: None,
                    capped: false,
                }],
                carried: Some(carried.clone()),
                exhausted: false,
            });
        }
        let per_slot = quick_craft_amount(carried, pointer.slots.len(), pointer.button);
        let mut remaining = carried.count;
        let mut slots = Vec::with_capacity(pointer.slots.len());
        for &slot in &pointer.slots {
            if !self.quick_slot_compatible(slot, carried) {
                continue;
            }
            let before = self.slot(slot).map_or(0, ItemStack::count);
            let uncapped = before.saturating_add(per_slot);
            let limit = if matches!(slot, PlayerSlot::Armor(_)) {
                1
            } else {
                carried.max_count?
            };
            let after = uncapped.min(limit);
            remaining = remaining.saturating_sub(after - before);
            let mut item = carried.clone();
            item.count = after;
            slots.push(QuickCraftSlotPreview {
                slot,
                item: Some(item),
                capped: uncapped > after,
            });
        }
        let exhausted = remaining == 0;
        let carried = (!exhausted).then(|| {
            let mut item = carried.clone();
            item.count = remaining;
            item
        });
        Some(QuickCraftPreview {
            slots,
            carried,
            exhausted,
        })
    }
    fn crafting_width(&self) -> usize {
        if matches!(self.menu, Some(InventoryMenu::CraftingTable { .. })) {
            3
        } else {
            2
        }
    }
    fn crafting_len(&self) -> usize {
        if matches!(self.menu, Some(InventoryMenu::Chest { .. })) {
            0
        } else {
            self.crafting_width().pow(2)
        }
    }
    /// Open a chest whose contents have been loaded by the session. A closed
    /// chest must be taken by its owner before another one can be opened.
    pub fn open_chest(&mut self, position: IVec3, slots: [Option<ItemStack>; 27]) -> bool {
        self.open_chest_slots(position, None, Vec::from(slots))
    }
    /// A double chest presents the right half first, followed by the left half.
    pub fn open_double_chest(
        &mut self,
        right: IVec3,
        left: IVec3,
        right_slots: [Option<ItemStack>; 27],
        left_slots: [Option<ItemStack>; 27],
    ) -> bool {
        if right == left {
            return false;
        }
        let mut slots = Vec::from(right_slots);
        slots.extend(left_slots);
        self.open_chest_slots(right, Some(left), slots)
    }
    fn open_chest_slots(
        &mut self,
        position: IVec3,
        other: Option<IVec3>,
        slots: Vec<Option<ItemStack>>,
    ) -> bool {
        if self.is_open()
            || self.chest.is_some()
            || self.cursor.is_some()
            || self.crafting.iter().any(Option::is_some)
        {
            return false;
        }
        self.chest = Some(ChestInventory {
            position,
            other,
            slots,
            title: None,
            title_component: None,
        });
        self.menu = Some(InventoryMenu::Chest { position, other });
        self.revision = self.revision.wrapping_add(1);
        true
    }
    /// The selected saved chest name, if its text component is supported.
    pub fn chest_title(&self) -> Option<&str> {
        if !matches!(self.menu, Some(InventoryMenu::Chest { .. })) {
            return None;
        }
        self.chest.as_ref()?.title.as_deref()
    }

    pub fn chest_title_component(&self) -> Option<&Tag> {
        if !matches!(self.menu, Some(InventoryMenu::Chest { .. })) {
            return None;
        }
        self.chest.as_ref()?.title_component.as_deref()
    }

    pub(crate) fn set_chest_title(&mut self, title: Option<Tag>) {
        if let Some(chest) = &mut self.chest {
            chest.title = match &title {
                Some(Tag::String(value)) => String::from_utf16(&value.0).ok(),
                _ => None,
            };
            chest.title_component = title.map(Arc::new);
            self.revision = self.revision.wrapping_add(1);
        }
    }
    /// Return the chest to the session after closing, including on death.
    pub fn has_closed_chest(&self) -> bool {
        !self.is_open() && self.chest.is_some()
    }
    pub fn take_closed_chest(&mut self) -> Option<(IVec3, Option<IVec3>, Vec<Option<ItemStack>>)> {
        if !self.has_closed_chest() {
            return None;
        }
        let chest = self.chest.take()?;
        self.revision = self.revision.wrapping_add(1);
        Some((chest.position, chest.other, chest.slots))
    }
    pub fn open_crafting_table(&mut self, position: IVec3) -> bool {
        if self.is_open()
            || self.chest.is_some()
            || self.cursor.is_some()
            || self.crafting.iter().any(Option::is_some)
        {
            return false;
        }
        self.menu = Some(InventoryMenu::CraftingTable { position });
        self.revision = self.revision.wrapping_add(1);
        true
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn select(&mut self, index: u8) -> bool {
        if index >= 9 || self.selected == index {
            return false;
        }
        self.selected = index;
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Select a matching stack, or create the picked stack in creative mode.
    /// Unknown saved components are left untouched when they could affect the choice.
    pub fn pick_item(&mut self, item: ItemStack, creative: bool) -> bool {
        if self.is_open() || item.max_count.is_none() {
            return false;
        }
        let mut found = None;
        for (index, stack) in self.storage.iter().enumerate() {
            if let Some(stack) = stack
                && stack.id == item.id
            {
                if stack.max_count.is_none() {
                    return false;
                }
                if stack.same_item_and_components(&item) {
                    found = Some(index);
                    break;
                }
            }
        }
        if let Some(index) = found
            && index < 9
        {
            return self.select(index as u8);
        }
        if found.is_none() && !creative {
            return false;
        }
        let indices = (0..9).map(|offset| (self.selected as usize + offset) % 9);
        let target = if let Some(empty) = indices.clone().find(|&i| self.storage[i].is_none()) {
            empty
        } else {
            let mut target = self.selected as usize;
            for index in indices {
                let stack = self.storage[index].as_ref().unwrap();
                if stack.max_count.is_none() {
                    return false;
                }
                match item_enchantments(stack) {
                    Ok(enchantments) if enchantments.is_empty() => {
                        target = index;
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => return false,
                }
            }
            target
        };
        self.selected = target as u8;
        if let Some(index) = found {
            self.storage.swap(target, index);
        } else {
            if self.storage[target].is_some()
                && let Some(free) = self.storage.iter().position(Option::is_none)
            {
                self.storage[free] = self.storage[target].take();
            }
            self.storage[target] = Some(item);
        }
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Exchange the selected hotbar stack and offhand outside a container.
    /// No item components need to be interpreted for this operation.
    pub fn swap_hands(&mut self) -> bool {
        if self.is_open()
            || (self.storage[self.selected as usize].is_none() && self.offhand.is_none())
        {
            return false;
        }
        std::mem::swap(&mut self.storage[self.selected as usize], &mut self.offhand);
        self.revision = self.revision.wrapping_add(1);
        true
    }

    fn mutable_slot(&mut self, slot: PlayerSlot) -> Option<&mut Option<ItemStack>> {
        let crafting_len = self.crafting_len();
        match slot {
            PlayerSlot::Storage(index) => self.storage.get_mut(index as usize),
            PlayerSlot::Crafting(index) if (index as usize) < crafting_len => {
                self.crafting.get_mut(index as usize)
            }
            PlayerSlot::Crafting(_) => None,
            PlayerSlot::Offhand => Some(&mut self.offhand),
            PlayerSlot::Armor(index) if matches!(self.menu, Some(InventoryMenu::Player)) => {
                self.armor.get_mut(index as usize)
            }
            PlayerSlot::Armor(_) | PlayerSlot::CraftResult => None,
            PlayerSlot::Chest(index) if matches!(self.menu, Some(InventoryMenu::Chest { .. })) => {
                self.chest.as_mut()?.slots.get_mut(index as usize)
            }
            PlayerSlot::Chest(_) => None,
        }
    }

    /// Apply a hotbar number or offhand key to a hovered player-inventory
    /// slot. Overflow is returned to the session for spawning as item entities.
    /// Item-specific overflow rules remain limited to known stack behavior.
    pub fn swap_slot(
        &mut self,
        source: PlayerSlot,
        target: PlayerSlot,
        creative: bool,
        recipes: Option<&CraftingRecipes>,
    ) -> Option<Vec<ItemStack>> {
        if !self.is_open()
            || self.cursor.is_some()
            || !matches!(target, PlayerSlot::Storage(0..=8) | PlayerSlot::Offhand)
        {
            return None;
        }
        if matches!(source, PlayerSlot::Crafting(_) | PlayerSlot::CraftResult) {
            self.clear_ghost_recipe();
        }
        if source == target {
            let item = self.slot(source)?;
            let limit = item.max_count?;
            if item.count <= limit || !ordinary_return_supported(item) {
                return None;
            }
            // Java's menu splits an oversized stack even when the clicked slot
            // and the selected hotbar/offhand slot refer to the same storage.
            let mut remainder = self.mutable_slot(source)?.take()?;
            let mut placed = remainder.clone();
            placed.count = limit;
            remainder.count -= limit;
            *self.mutable_slot(source)? = Some(placed);
            let dropped = self
                .insert_returned(remainder)
                .filter(|_| !creative)
                .into_iter()
                .collect();
            self.revision = self.revision.wrapping_add(1);
            return Some(dropped);
        }
        // ResultSlot only allows taking its output into an empty target and
        // invokes crafting callbacks; supported recipes already have a handler.
        if source == PlayerSlot::CraftResult {
            if self.slot(target).is_some() || !self.take_crafting_result(recipes?) {
                return None;
            }
            let output = self.cursor.take();
            *self.mutable_slot(target)? = output;
            return Some(Vec::new());
        }
        if let PlayerSlot::Armor(index) = source
            && (!matches!(self.menu, Some(InventoryMenu::Player))
                || armor_slot(index).is_none()
                || !self.can_take_equipment(self.slot(source), creative)
                || self.slot(target).is_some_and(|item| {
                    !basic_pickup_supported(item) || !self.armor_can_accept(index, item)
                }))
        {
            return None;
        }
        // Reject malformed indices before moving either stack.
        if !matches!(source, PlayerSlot::Storage(0..=35) | PlayerSlot::Offhand)
            && !matches!(source, PlayerSlot::Crafting(index) if (index as usize) < self.crafting_len())
            && !matches!(source, PlayerSlot::Armor(0..=3))
            && !(matches!(self.menu, Some(InventoryMenu::Chest { .. }))
                && matches!(source, PlayerSlot::Chest(index) if self.chest.as_ref().is_some_and(|chest| usize::from(index) < chest.slots.len())))
        {
            return None;
        }
        let incoming = self.slot(target);
        let outgoing = self.slot(source);
        if matches!(self.menu, Some(InventoryMenu::CraftingTable { .. }))
            && matches!(source, PlayerSlot::Crafting(_))
            && incoming.is_some_and(|item| !ordinary_return_supported(item))
        {
            return None;
        }
        if incoming.is_none() && outgoing.is_none() {
            return None;
        }
        let limit = match incoming {
            Some(item) if item.count == 1 && item.max_count.is_none() => 1,
            Some(item) => item.max_count?,
            None => 99,
        };
        let limit = if matches!(source, PlayerSlot::Armor(_)) {
            limit.min(1)
        } else {
            limit
        };
        let split = incoming.is_some_and(|item| item.count > limit);
        // Inventory.add has item-dependent behavior. Until those components
        // are resolved, don't risk discarding or misplacing overflow.
        if split && outgoing.is_some_and(|item| !basic_pickup_supported(item)) {
            return None;
        }
        let mut incoming = self.mutable_slot(target)?.take();
        let outgoing = self.mutable_slot(source)?.take();
        let mut dropped = Vec::new();
        if split {
            let remainder = incoming.as_mut().expect("split stack exists");
            let mut placed = remainder.clone();
            placed.count = limit;
            remainder.count -= limit;
            *self.mutable_slot(source)? = Some(placed);
            *self.mutable_slot(target)? = incoming;
            if let Some(item) = outgoing
                && let Some(remainder) = self.insert_returned(item)
                && !creative
            {
                dropped.push(remainder);
            }
        } else {
            *self.mutable_slot(source)? = incoming;
            *self.mutable_slot(target)? = outgoing;
        }
        self.revision = self.revision.wrapping_add(1);
        if matches!(source, PlayerSlot::Crafting(_))
            && let Some(recipes) = recipes
        {
            self.refresh_crafting(recipes);
        }
        Some(dropped)
    }

    /// Consume the selected item after a successful survival placement.
    pub fn consume_selected(&mut self) -> bool {
        self.consume_held(PlayerSlot::Storage(self.selected))
    }

    /// Damage one supported ordinary tool, preserving its other components.
    pub(crate) fn damage_selected_tool(&mut self, amount: u32, random: &mut LegacyRandom) -> bool {
        self.damage_held_tool(PlayerSlot::Storage(self.selected), amount, random)
    }

    /// Damage the selected main-hand or offhand tool after a successful use.
    pub(crate) fn damage_held_tool(
        &mut self,
        hand: PlayerSlot,
        amount: u32,
        random: &mut LegacyRandom,
    ) -> bool {
        if amount == 0 {
            return false;
        }
        if !matches!(hand, PlayerSlot::Offhand)
            && !matches!(hand, PlayerSlot::Storage(index) if index == self.selected)
        {
            return false;
        }
        let Some(item) = self.slot(hand) else {
            return false;
        };
        if item.count != 1 || !ordinary_return_supported(item) {
            return false;
        }
        let Ok(enchantments) = item_enchantments(item) else {
            return false;
        };
        let mut next_random = random.clone();
        let mut damage = amount as f32;
        let mut effect_count = 0;
        for (id, level) in enchantments {
            let Some(registry) = &self.enchantments else {
                return false;
            };
            let Ok((next, has_effect)) =
                registry.apply_item_damage(&id, level, item.id(), damage, &mut || {
                    next_random.next_float()
                })
            else {
                return false;
            };
            if has_effect {
                effect_count += 1;
            }
            // The Java enchantment map determines the order of multiple
            // independently effective enchantments; it is not NBT order.
            if effect_count > 1 {
                return false;
            }
            damage = next;
        }
        let amount = damage as i32;
        *random = next_random;
        if amount == 0 {
            return false;
        }
        let slot = match hand {
            PlayerSlot::Storage(index) if index == self.selected => {
                &mut self.storage[index as usize]
            }
            PlayerSlot::Offhand => &mut self.offhand,
            _ => return false,
        };
        let Some(item) = slot.as_mut() else {
            return false;
        };
        let Some(maximum) = ordinary_item_default_durability(item.id()) else {
            return false;
        };
        if item.count != 1 || !ordinary_return_supported(item) {
            return false;
        }
        let damage = item
            .components
            .get("minecraft:damage")
            .and_then(Tag::as_java_int)
            .unwrap_or(0) as i64;
        let next = (damage + i64::from(amount)).max(0);
        if next >= i64::from(maximum) {
            *slot = None;
        } else if let Tag::Compound(components) = &mut item.components {
            if let Some((_, value)) = components
                .iter_mut()
                .find(|(name, _)| name.matches("minecraft:damage"))
            {
                *value = Tag::Int(next as i32);
            } else {
                components.push(("minecraft:damage".into(), Tag::Int(next as i32)));
            }
        } else {
            return false;
        }
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Consume the item in the hand that performed a successful placement.
    pub fn consume_held(&mut self, hand: PlayerSlot) -> bool {
        let slot = match hand {
            PlayerSlot::Storage(index) if index == self.selected => {
                &mut self.storage[index as usize]
            }
            PlayerSlot::Offhand => &mut self.offhand,
            _ => return false,
        };
        let Some(item) = slot.as_mut() else {
            return false;
        };
        if item.max_count.is_none() {
            return false;
        }
        item.count -= 1;
        if item.count == 0 {
            *slot = None;
        }
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Add a supported dropped stack using Java's selected-slot, offhand,
    /// other-stack, then empty-slot order. The remainder stays in `source`.
    pub fn pickup_stack(&mut self, source: &mut Option<ItemStack>, creative: bool) -> bool {
        self.pickup_stack_with_transfer(source, creative).0
    }

    /// Return Java's success flag and the amount actually inserted. Creative
    /// mode can clear a remainder without inserting it into the inventory.
    pub(crate) fn pickup_stack_with_transfer(
        &mut self,
        source: &mut Option<ItemStack>,
        creative: bool,
    ) -> (bool, u32) {
        let Some(mut item) = source.take() else {
            return (false, 0);
        };
        if !basic_pickup_supported(&item) {
            *source = Some(item);
            return (false, 0);
        }
        let original = item.count;
        let max = item.max_count.expect("resolved stack");
        loop {
            let has_room = |existing: &ItemStack| {
                existing.same_item_and_components(&item) && existing.count < max
            };
            let selected = self.selected as usize;
            let target = if self.storage[selected].as_ref().is_some_and(has_room) {
                Some(&mut self.storage[selected])
            } else if self.offhand.as_ref().is_some_and(has_room) {
                Some(&mut self.offhand)
            } else if let Some(index) = self
                .storage
                .iter()
                .position(|stack| stack.as_ref().is_some_and(has_room))
            {
                Some(&mut self.storage[index])
            } else if let Some(index) = self.storage.iter().position(Option::is_none) {
                Some(&mut self.storage[index])
            } else {
                None
            };
            let Some(target) = target else {
                break;
            };
            let amount = match target {
                Some(existing) => {
                    let amount = item.count.min(max - existing.count);
                    existing.count += amount;
                    amount
                }
                None => {
                    let amount = item.count.min(max);
                    let mut placed = item.clone();
                    placed.count = amount;
                    *target = Some(placed);
                    amount
                }
            };
            item.count -= amount;
            if item.count == 0 {
                break;
            }
        }
        let transferred = original - item.count;
        if transferred > 0 {
            self.revision = self.revision.wrapping_add(1);
        }
        let success = item.count == 0 || creative;
        if creative {
            item.count = 0;
        }
        *source = (item.count != 0).then_some(item);
        (success, transferred)
    }

    /// Toggle without an item-entity sink. A close that needs drops is refused.
    pub fn toggle(&mut self) -> bool {
        let mut next = self.clone();
        let Some(drops) = next.toggle_with_drops() else {
            return false;
        };
        if !drops.is_empty() {
            return false;
        }
        *self = next;
        true
    }

    /// Close by returning the cursor and crafting grid in order. Any overflow
    /// is returned to the session so it can be spawned as item entities.
    pub fn toggle_with_drops(&mut self) -> Option<Vec<ItemStack>> {
        if !self.is_open() {
            if self.chest.is_some() {
                return None;
            }
            self.menu = Some(InventoryMenu::Player);
            self.revision = self.revision.wrapping_add(1);
            return Some(Vec::new());
        }
        let mut next = self.clone();
        let mut returned = Vec::new();
        if let Some(stack) = next.cursor.take() {
            returned.push(stack);
        }
        for slot in &mut next.crafting {
            if let Some(stack) = slot.take() {
                returned.push(stack);
            }
        }
        next.craft_result = None;
        next.ghost_recipe = None;
        next.pointer = None;
        if returned.iter().any(|item| !basic_pickup_supported(item)) {
            return None;
        }
        let mut drops = Vec::new();
        for stack in returned {
            if let Some(remainder) = next.insert_returned(stack) {
                drops.push(remainder);
            }
        }
        next.menu = None;
        next.revision = self.revision.wrapping_add(1);
        *self = next;
        Some(drops)
    }

    /// Apply inventory death drops, then close the active container. Evaluate
    /// enchantments and the container return on a clone first, so unsupported
    /// components cannot leave half of an inventory modified.
    pub(crate) fn take_death_drops(&mut self, keep_inventory: bool) -> anyhow::Result<DeathDrops> {
        use anyhow::{Context, ensure};
        let mut next = self.clone();
        let mut vanishing = Vec::new();
        if !keep_inventory {
            for item in self
                .storage
                .iter()
                .chain(std::iter::once(&self.offhand))
                .chain(self.armor.iter().rev())
            {
                let mut prevents_drop = false;
                if let Some(enchantments) = item
                    .as_ref()
                    .and_then(|stack| stack.components.get("minecraft:enchantments"))
                {
                    let Tag::Compound(entries) = enchantments else {
                        anyhow::bail!("item enchantments are not a compound");
                    };
                    for (name, level) in entries {
                        let name = String::from_utf16(&name.0).context("invalid enchantment id")?;
                        let id = ResourceId::parse(&name)?;
                        ensure!(
                            level
                                .as_java_int()
                                .is_some_and(|value| (1..=255).contains(&value)),
                            "invalid enchantment level"
                        );
                        let registry = self
                            .enchantments
                            .as_ref()
                            .context("active enchantment registry is unavailable")?;
                        prevents_drop |= registry.prevents_equipment_drop(&id)?;
                    }
                }
                vanishing.push(prevents_drop);
            }
        }
        let mut drops = Vec::new();
        if !keep_inventory {
            // EntityEquipment.dropAll iterates OFFHAND, FEET, LEGS, CHEST,
            // HEAD after the 36 ordinary slots. Armor is stored head first here.
            for (slot, vanish) in next
                .storage
                .iter_mut()
                .chain(std::iter::once(&mut next.offhand))
                .chain(next.armor.iter_mut().rev())
                .zip(vanishing)
            {
                if let Some(stack) = slot.take()
                    && !vanish
                {
                    drops.push(stack);
                }
            }
        }
        let container_overflow = if next.is_open() {
            next.toggle_with_drops()
                .context("cannot safely return the open container's items")?
        } else {
            ensure!(
                next.cursor.is_none() && next.crafting.iter().all(Option::is_none),
                "closed container has unhandled items"
            );
            Vec::new()
        };
        let retained_without_keep = !keep_inventory
            && (next.storage.iter().any(Option::is_some)
                || next.armor.iter().any(Option::is_some)
                || next.offhand.is_some());
        next.revision = self.revision.wrapping_add(1);
        *self = next;
        Ok(DeathDrops {
            inventory: drops,
            container_overflow,
            retained_without_keep,
        })
    }

    fn insert_returned(&mut self, mut item: ItemStack) -> Option<ItemStack> {
        let Some(max) = item.max_count else {
            return Some(item);
        };
        loop {
            let has_room = |existing: &ItemStack| {
                existing.same_item_and_components(&item) && existing.count < max
            };
            let selected = self.selected as usize;
            let target = if self.storage[selected].as_ref().is_some_and(has_room) {
                Some(&mut self.storage[selected])
            } else if self.offhand.as_ref().is_some_and(has_room) {
                Some(&mut self.offhand)
            } else if let Some(index) = self
                .storage
                .iter()
                .position(|stack| stack.as_ref().is_some_and(has_room))
            {
                Some(&mut self.storage[index])
            } else if let Some(index) = self.storage.iter().position(Option::is_none) {
                Some(&mut self.storage[index])
            } else {
                None
            };
            let Some(target) = target else {
                return Some(item);
            };
            let amount = match target {
                Some(existing) => {
                    let amount = item.count.min(max - existing.count);
                    existing.count += amount;
                    amount
                }
                None => {
                    let amount = item.count.min(max);
                    let mut placed = item.clone();
                    placed.count = amount;
                    *target = Some(placed);
                    amount
                }
            };
            item.count -= amount;
            if item.count == 0 {
                return None;
            }
        }
    }

    /// Split the selected hotbar stack for Q or Ctrl+Q while playing.
    pub fn drop_selected(&mut self, all: bool) -> Option<ItemStack> {
        if self.is_open() {
            return None;
        }
        let item = split_supported(&mut self.storage[self.selected as usize], all)?;
        self.revision = self.revision.wrapping_add(1);
        Some(item)
    }

    /// Q over a normal inventory slot works only when no stack is carried.
    pub fn throw_slot(&mut self, slot: PlayerSlot, all: bool) -> Option<ItemStack> {
        self.throw_slot_with_mode(slot, all, false)
    }

    pub fn throw_slot_with_mode(
        &mut self,
        slot: PlayerSlot,
        all: bool,
        creative: bool,
    ) -> Option<ItemStack> {
        if !self.is_open() || self.cursor.is_some() {
            return None;
        }
        if !matches!(self.menu, Some(InventoryMenu::Player))
            && matches!(slot, PlayerSlot::Offhand | PlayerSlot::Armor(_))
        {
            return None;
        }
        if let PlayerSlot::Armor(index) = slot
            && (!matches!(self.menu, Some(InventoryMenu::Player))
                || !self.can_take_equipment(self.armor.get(index as usize)?.as_ref(), creative))
        {
            return None;
        }
        let crafting_len = self.crafting_len();
        let target = match slot {
            PlayerSlot::Storage(index) => self.storage.get_mut(index as usize),
            PlayerSlot::Crafting(index) if (index as usize) < crafting_len => {
                self.crafting.get_mut(index as usize)
            }
            PlayerSlot::Crafting(_) => None,
            PlayerSlot::Offhand => Some(&mut self.offhand),
            PlayerSlot::Armor(index) => self.armor.get_mut(index as usize),
            PlayerSlot::CraftResult => None,
            PlayerSlot::Chest(index) if matches!(self.menu, Some(InventoryMenu::Chest { .. })) => {
                self.chest.as_mut()?.slots.get_mut(index as usize)
            }
            PlayerSlot::Chest(_) => None,
        }?;
        let item = split_supported(target, all)?;
        self.revision = self.revision.wrapping_add(1);
        Some(item)
    }

    /// Clicking outside the inventory drops all carried items on left click,
    /// or one item on right click.
    pub fn drop_cursor(&mut self, button: ClickButton) -> Option<ItemStack> {
        if !self.is_open() {
            return None;
        }
        let item = split_supported(&mut self.cursor, button == ClickButton::Primary)?;
        self.revision = self.revision.wrapping_add(1);
        Some(item)
    }

    pub fn set_slot(&mut self, slot: PlayerSlot, value: Option<ItemStack>) -> bool {
        let crafting_len = self.crafting_len();
        let target = match slot {
            PlayerSlot::Storage(index) => self.storage.get_mut(index as usize),
            PlayerSlot::Crafting(index) if (index as usize) < crafting_len => {
                self.crafting.get_mut(index as usize)
            }
            PlayerSlot::Crafting(_) => None,
            PlayerSlot::CraftResult => None,
            PlayerSlot::Armor(index) => self.armor.get_mut(index as usize),
            PlayerSlot::Offhand => Some(&mut self.offhand),
            PlayerSlot::Chest(index) if matches!(self.menu, Some(InventoryMenu::Chest { .. })) => {
                self.chest
                    .as_mut()
                    .and_then(|chest| chest.slots.get_mut(index as usize))
            }
            PlayerSlot::Chest(_) => None,
        };
        let Some(target) = target else {
            return false;
        };
        if *target == value {
            return false;
        }
        *target = value;
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// Handle ordinary slots and known equipment-slot rules.
    pub fn click(&mut self, slot: PlayerSlot, button: ClickButton) -> bool {
        self.click_with_mode(slot, button, false)
    }

    pub fn click_with_mode(
        &mut self,
        slot: PlayerSlot,
        button: ClickButton,
        creative: bool,
    ) -> bool {
        if !self.is_open() {
            return false;
        }
        let rules = match slot {
            PlayerSlot::Armor(index) => {
                if !matches!(self.menu, Some(InventoryMenu::Player)) || armor_slot(index).is_none()
                {
                    return false;
                }
                SlotRules {
                    max_count: 1,
                    may_take: self
                        .can_take_equipment(self.armor[index as usize].as_ref(), creative),
                    accepts_carried: self
                        .cursor
                        .as_ref()
                        .is_some_and(|item| self.armor_can_accept(index, item)),
                    accepts_existing: true,
                }
            }
            PlayerSlot::Offhand if !matches!(self.menu, Some(InventoryMenu::Player)) => {
                return false;
            }
            _ => SlotRules::STANDARD,
        };
        let crafting_len = self.crafting_len();
        let target = match slot {
            PlayerSlot::Storage(index) => self.storage.get_mut(index as usize),
            PlayerSlot::Crafting(index) if (index as usize) < crafting_len => {
                self.crafting.get_mut(index as usize)
            }
            PlayerSlot::Crafting(_) => None,
            PlayerSlot::CraftResult => None,
            PlayerSlot::Offhand => Some(&mut self.offhand),
            PlayerSlot::Armor(index) => self.armor.get_mut(index as usize),
            PlayerSlot::Chest(index) if matches!(self.menu, Some(InventoryMenu::Chest { .. })) => {
                self.chest
                    .as_mut()
                    .and_then(|chest| chest.slots.get_mut(index as usize))
            }
            PlayerSlot::Chest(_) => None,
        };
        let Some(target) = target else {
            return false;
        };
        if target
            .as_ref()
            .is_some_and(|item| !basic_pickup_supported(item))
            || self
                .cursor
                .as_ref()
                .is_some_and(|item| !basic_pickup_supported(item))
        {
            return false;
        }
        if pickup(target, &mut self.cursor, button, rules) {
            self.revision = self.revision.wrapping_add(1);
            true
        } else {
            false
        }
    }

    /// Shift-click a supported ordinary stack between the player inventory
    /// sections using InventoryMenu's merge-first ordering.
    pub fn quick_move(&mut self, slot: PlayerSlot) -> bool {
        self.quick_move_with_mode(slot, false)
    }

    pub fn quick_move_with_mode(&mut self, slot: PlayerSlot, creative: bool) -> bool {
        if !self.is_open() {
            return false;
        }
        if matches!(self.menu, Some(InventoryMenu::Chest { .. })) {
            return self.quick_move_chest(slot);
        }
        let table = matches!(self.menu, Some(InventoryMenu::CraftingTable { .. }));
        if table && matches!(slot, PlayerSlot::Offhand | PlayerSlot::Armor(_)) {
            return false;
        }
        if let PlayerSlot::Armor(index) = slot {
            let Some(equipped) = self.armor.get(index as usize) else {
                return false;
            };
            if !self.can_take_equipment(equipped.as_ref(), creative) {
                return false;
            }
        }
        let crafting_len = self.crafting_len();
        let source = match slot {
            PlayerSlot::Storage(index) => self.storage.get_mut(index as usize),
            PlayerSlot::Crafting(index) if (index as usize) < crafting_len => {
                self.crafting.get_mut(index as usize)
            }
            PlayerSlot::Crafting(_) => None,
            PlayerSlot::CraftResult => None,
            PlayerSlot::Offhand => Some(&mut self.offhand),
            PlayerSlot::Armor(index) => self.armor.get_mut(index as usize),
            PlayerSlot::Chest(_) => None,
        };
        let Some(source) = source else {
            return false;
        };
        let Some(mut item) = source.take() else {
            return false;
        };
        if !basic_pickup_supported(&item) || item.max_count.is_none() {
            *source = Some(item);
            return false;
        }
        let before = item.count;
        if !table && matches!(slot, PlayerSlot::Storage(_)) {
            let destination = match crate::equippable::preferred_slot(&item) {
                Some(EquipmentSlot::Head) => Some(PlayerSlot::Armor(0)),
                Some(EquipmentSlot::Chest) => Some(PlayerSlot::Armor(1)),
                Some(EquipmentSlot::Legs) => Some(PlayerSlot::Armor(2)),
                Some(EquipmentSlot::Feet) => Some(PlayerSlot::Armor(3)),
                Some(EquipmentSlot::Offhand) => Some(PlayerSlot::Offhand),
                _ => None,
            };
            if let Some(destination) = destination
                && self.slot(destination).is_none()
            {
                if matches!(destination, PlayerSlot::Armor(index) if !self.armor_can_accept(index, &item))
                {
                    *self.mutable_slot(slot).expect("checked source") = Some(item);
                    return false;
                }
                let limit = if matches!(destination, PlayerSlot::Armor(_)) {
                    1
                } else {
                    item.max_count.expect("resolved stack")
                };
                let amount = item.count.min(limit);
                let mut placed = item.clone();
                placed.count = amount;
                item.count -= amount;
                let remainder = item.count > 0;
                *self
                    .mutable_slot(destination)
                    .expect("checked equipment destination") = Some(placed);
                *self.mutable_slot(slot).expect("checked source") =
                    (item.count > 0).then_some(item);
                self.revision = self.revision.wrapping_add(1);
                // QUICK_MOVE repeats while an original stack remains in place.
                if remainder {
                    self.quick_move_with_mode(slot, creative);
                }
                return true;
            }
        }
        let indexes: Vec<usize> = match slot {
            PlayerSlot::Storage(index) if index < 9 => (9..36).collect(),
            PlayerSlot::Storage(_) => (0..9).collect(),
            PlayerSlot::Crafting(_) | PlayerSlot::Offhand | PlayerSlot::Armor(_) => {
                (9..36).chain(0..9).collect()
            }
            PlayerSlot::CraftResult => unreachable!(),
            PlayerSlot::Chest(_) => unreachable!(),
        };
        let craft_indexes: Vec<usize> = (0..crafting_len).collect();
        loop {
            let to_crafting = table && matches!(slot, PlayerSlot::Storage(_));
            let moved = (to_crafting
                && move_stack_to(&mut self.crafting, &craft_indexes, &mut item))
                || move_stack_to(&mut self.storage, &indexes, &mut item);
            if !moved || item.count == 0 || !table {
                break;
            }
        }
        let source = match slot {
            PlayerSlot::Storage(index) => &mut self.storage[index as usize],
            PlayerSlot::Crafting(index) => &mut self.crafting[index as usize],
            PlayerSlot::Offhand => &mut self.offhand,
            PlayerSlot::Armor(index) => &mut self.armor[index as usize],
            PlayerSlot::CraftResult => unreachable!(),
            PlayerSlot::Chest(_) => unreachable!(),
        };
        let changed = item.count != before;
        *source = (item.count != 0).then_some(item);
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    fn quick_move_chest(&mut self, slot: PlayerSlot) -> bool {
        let Some(chest) = self.chest.as_mut() else {
            return false;
        };
        let chest_len = chest.slots.len();
        let (source, destination, indexes): (
            &mut Option<ItemStack>,
            &mut [Option<ItemStack>],
            Vec<usize>,
        ) = match slot {
            PlayerSlot::Chest(index) if usize::from(index) < chest_len => (
                &mut chest.slots[index as usize],
                &mut self.storage,
                (0..9).rev().chain((9..36).rev()).collect(),
            ),
            PlayerSlot::Storage(index) if index < 36 => (
                &mut self.storage[index as usize],
                &mut chest.slots,
                (0..chest_len).collect(),
            ),
            _ => return false,
        };
        let Some(mut item) = source.take() else {
            return false;
        };
        if !basic_pickup_supported(&item) {
            *source = Some(item);
            return false;
        }
        let changed = move_stack_to(destination, &indexes, &mut item);
        *source = (item.count != 0).then_some(item);
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }

    /// Refresh a result only where input and output item behavior is known.
    pub fn refresh_crafting(&mut self, recipes: &CraftingRecipes) {
        if matches!(self.menu, Some(InventoryMenu::Chest { .. })) {
            return;
        }
        let width = self.crafting_width();
        let grid = &self.crafting[..width * width];
        let safe = grid
            .iter()
            .flatten()
            .all(|item| basic_pickup_supported(item) && ordinary_return_supported(item));
        let result = if safe {
            let slots: Vec<Option<&str>> = grid
                .iter()
                .map(|slot| slot.as_ref().map(ItemStack::id))
                .collect();
            recipes.matching(width, width, &slots).and_then(|recipe| {
                let id = format!("{}:{}", recipe.result.id.namespace, recipe.result.id.path);
                let max = ordinary_item_default_max(&id)?;
                let empty_components = recipe
                    .result
                    .components
                    .as_ref()
                    .is_none_or(|value| value.as_object().is_some_and(|map| map.is_empty()));
                if !empty_components || recipe.result.count > max {
                    return None;
                }
                ItemStack::new(id, recipe.result.count, max, Tag::Compound(vec![]))
            })
        } else {
            None
        };
        if self.craft_result != result {
            self.craft_result = result;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Apply a known recipe-book entry to the current crafting grid. The
    /// transaction is refused if the existing grid cannot be returned safely.
    pub fn place_book_recipe(
        &mut self,
        recipe: &CraftingRecipe,
        recipes: &CraftingRecipes,
        use_max: bool,
    ) -> RecipeBookPlacement {
        if !self.is_open()
            || matches!(self.menu, Some(InventoryMenu::Chest { .. }))
            || !self.recipe_book.known.contains(&recipe.id)
        {
            return RecipeBookPlacement::Unchanged;
        }
        let width = self.crafting_width();
        if recipe.book_layout(width, width).is_none() {
            return RecipeBookPlacement::Unchanged;
        }
        let stacks = self.recipe_book_available();
        let available = RecipeBookInventory::new(&stacks);
        let assignment = |amount| {
            recipe.book_ingredients_counted(width, width, &recipes.tags, &available, amount)
        };
        let mut lo = 0;
        let mut hi = 100;
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if assignment(mid).is_some() {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let grid: Vec<_> = self.crafting[..width * width]
            .iter()
            .map(|slot| slot.as_ref().map(ItemStack::id))
            .collect();
        let matches = recipe.matches(width, width, &grid, &recipes.tags);
        if lo > 0
            && matches
            && self.crafting[..width * width]
                .iter()
                .flatten()
                .any(|item| item.count.saturating_add(1) > lo.min(item.max_count.unwrap_or(0)))
        {
            return RecipeBookPlacement::Unchanged;
        }
        let amount = if use_max {
            lo
        } else if matches {
            self.crafting[..width * width]
                .iter()
                .flatten()
                .map(ItemStack::count)
                .min()
                .unwrap_or(0)
                .saturating_add(1)
        } else {
            1
        };
        let mut selected = (lo > 0).then(|| assignment(amount)).flatten();
        let mut selected_amount = amount;
        if let Some(items) = &selected {
            for id in items.iter().flatten() {
                selected_amount = selected_amount.min(ordinary_item_default_max(id).unwrap_or(0));
            }
            if selected_amount != amount {
                selected = (selected_amount > 0)
                    .then(|| assignment(selected_amount))
                    .flatten();
            }
        }
        drop(available);
        drop(stacks);
        let mut next = self.clone();
        for index in 0..width * width {
            if let Some(stack) = next.crafting[index].take()
                && (!ordinary_return_supported(&stack) || next.insert_returned(stack).is_some())
            {
                return RecipeBookPlacement::Unchanged;
            }
        }
        next.craft_result = None;
        let Some(selected) = selected else {
            next.ghost_recipe = Some(recipe.id.clone());
            next.revision = self.revision.wrapping_add(1);
            *self = next;
            return RecipeBookPlacement::Ghost;
        };
        for (index, id) in selected.into_iter().enumerate() {
            let Some(id) = id else {
                continue;
            };
            let mut remaining = selected_amount;
            while remaining > 0 {
                let source = next.storage.iter().position(|slot| {
                    slot.as_ref().is_some_and(|item| {
                        item.id() == id
                            && recipe_book_usable(item)
                            && next.crafting[index]
                                .as_ref()
                                .is_none_or(|placed| placed.same_item_and_components(item))
                    })
                });
                let Some(source) = source else {
                    break;
                };
                let item = next.storage[source].as_mut().expect("located item");
                let take = remaining.min(item.count);
                let mut part = item.clone();
                part.count = take;
                item.count -= take;
                if item.count == 0 {
                    next.storage[source] = None;
                }
                if let Some(placed) = &mut next.crafting[index] {
                    placed.count += take;
                } else {
                    next.crafting[index] = Some(part);
                }
                remaining -= take;
            }
        }
        next.ghost_recipe = None;
        next.revision = self.revision.wrapping_add(1);
        next.refresh_crafting(recipes);
        *self = next;
        RecipeBookPlacement::Placed
    }

    /// A vanilla result slot yields the whole output even for a secondary click.
    pub fn take_crafting_result(&mut self, recipes: &CraftingRecipes) -> bool {
        if !self.is_open() || matches!(self.menu, Some(InventoryMenu::Chest { .. })) {
            return false;
        }
        self.refresh_crafting(recipes);
        let Some(output) = self.craft_result.clone() else {
            return false;
        };
        match &mut self.cursor {
            None => self.cursor = Some(output),
            Some(cursor)
                if cursor.same_item_and_components(&output)
                    && cursor.count.saturating_add(output.count)
                        <= cursor.max_count.expect("resolved output") =>
            {
                cursor.count += output.count;
            }
            Some(_) => return false,
        }
        self.consume_crafting_ingredients(recipes);
        true
    }

    fn consume_crafting_ingredients(&mut self, recipes: &CraftingRecipes) {
        for ingredient in &mut self.crafting {
            if let Some(item) = ingredient {
                item.count -= 1;
                if item.count == 0 {
                    *ingredient = None;
                }
            }
        }
        self.revision = self.revision.wrapping_add(1);
        self.refresh_crafting(recipes);
    }

    /// Shift-click the result of an open crafting menu, including partial
    /// output that vanilla drops when the inventory cannot accept it all.
    pub fn quick_crafting_result(&mut self, recipes: &CraftingRecipes) -> Option<Vec<ItemStack>> {
        if !self.is_open() || matches!(self.menu, Some(InventoryMenu::Chest { .. })) {
            return None;
        }
        self.refresh_crafting(recipes);
        let initial = self.craft_result.as_ref()?.id.clone();
        let indexes: Vec<usize> = (0..9).rev().chain((9..36).rev()).collect();
        let mut drops = Vec::new();
        let mut moved_any = false;
        // A legal stack has at most 99 items; cap malformed oversized input so
        // one click cannot monopolize the fixed-tick loop.
        for _ in 0..99 {
            let Some(mut output) = self.craft_result.clone().filter(|item| item.id == initial)
            else {
                break;
            };
            if !move_stack_to(&mut self.storage, &indexes, &mut output) {
                break;
            }
            moved_any = true;
            if output.count > 0 {
                drops.push(output);
            }
            self.consume_crafting_ingredients(recipes);
        }
        moved_any.then_some(drops)
    }
}

fn move_stack_to(slots: &mut [Option<ItemStack>], indexes: &[usize], item: &mut ItemStack) -> bool {
    let Some(max) = item.max_count else {
        return false;
    };
    let before = item.count;
    for &index in indexes {
        if let Some(existing) = &mut slots[index]
            && existing.same_item_and_components(item)
        {
            let amount = item.count.min(max.saturating_sub(existing.count));
            existing.count += amount;
            item.count -= amount;
            if item.count == 0 {
                break;
            }
        }
    }
    if item.count != 0
        && let Some(index) = indexes
            .iter()
            .copied()
            .find(|&index| slots[index].is_none())
    {
        let amount = item.count.min(max);
        let mut moved = item.clone();
        moved.count = amount;
        slots[index] = Some(moved);
        item.count -= amount;
    }
    item.count != before
}

fn basic_pickup_supported(item: &ItemStack) -> bool {
    item.max_count.is_some()
        && crate::equippable::saved_inventory_max(item.id(), &item.components) == item.max_count
        && max_component_consistent(item)
}

fn recipe_book_usable(item: &ItemStack) -> bool {
    ordinary_return_supported(item)
        && item
            .components
            .get("minecraft:damage")
            .and_then(Tag::as_java_int)
            .is_none_or(|damage| damage == 0)
}

pub(crate) fn ordinary_return_supported(item: &ItemStack) -> bool {
    item.max_count.is_some()
        && saved_item_max(item.id(), &item.components) == item.max_count
        && max_component_consistent(item)
}

fn split_supported(source: &mut Option<ItemStack>, all: bool) -> Option<ItemStack> {
    let stack = source.as_mut()?;
    if stack.max_count.is_none() || !basic_pickup_supported(stack) {
        return None;
    }
    let count = if all { stack.count } else { 1 };
    let mut dropped = stack.clone();
    dropped.count = count;
    stack.count -= count;
    if stack.count == 0 {
        *source = None;
    }
    Some(dropped)
}

pub fn basic_item_max(id: &str) -> Option<u32> {
    match id {
        "minecraft:grass_block"
        | "minecraft:dirt"
        | "minecraft:coarse_dirt"
        | "minecraft:rooted_dirt"
        | "minecraft:podzol"
        | "minecraft:mycelium"
        | "minecraft:dirt_path"
        | "minecraft:farmland"
        | "minecraft:stone"
        | "minecraft:stone_bricks"
        | "minecraft:stone_button"
        | "minecraft:lever"
        | "minecraft:redstone_block"
        | "minecraft:lodestone"
        | "minecraft:redstone_lamp"
        | "minecraft:coal_ore"
        | "minecraft:deepslate_coal_ore"
        | "minecraft:iron_ore"
        | "minecraft:deepslate_iron_ore"
        | "minecraft:copper_ore"
        | "minecraft:deepslate_copper_ore"
        | "minecraft:gold_ore"
        | "minecraft:deepslate_gold_ore"
        | "minecraft:diamond_ore"
        | "minecraft:deepslate_diamond_ore"
        | "minecraft:emerald_ore"
        | "minecraft:deepslate_emerald_ore"
        | "minecraft:lapis_ore"
        | "minecraft:deepslate_lapis_ore"
        | "minecraft:redstone_ore"
        | "minecraft:deepslate_redstone_ore"
        | "minecraft:nether_gold_ore"
        | "minecraft:nether_quartz_ore"
        | "minecraft:snow"
        | "minecraft:snow_block" => Some(64),
        _ => None,
    }
}

// These families are implemented as game behavior, not as a generated copy of
// the installed registry. Unknown items remain unresolved until their item
// semantics (including stack limits and special click handlers) are available.
const TOOL_MATERIALS: &[&str] = &[
    "wooden",
    "stone",
    "copper",
    "iron",
    "golden",
    "diamond",
    "netherite",
];
const TOOL_KINDS: &[&str] = &["sword", "shovel", "pickaxe", "axe", "hoe"];
const WOODS: &[&str] = &[
    "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak",
    "poplar",
];
const STACKABLE_ITEMS: &[&str] = &[
    "cobblestone",
    "sand",
    "red_sand",
    "gravel",
    "glass",
    "crafting_table",
    "furnace",
    "chest",
    "trapped_chest",
    "torch",
    "hanging_roots",
    "bamboo_planks",
    "bamboo_mosaic",
    "bamboo_block",
    "stripped_bamboo_block",
    "crimson_planks",
    "warped_planks",
    "crimson_stem",
    "warped_stem",
    "stripped_crimson_stem",
    "stripped_warped_stem",
    "crimson_hyphae",
    "warped_hyphae",
    "stripped_crimson_hyphae",
    "stripped_warped_hyphae",
    "stick",
    "bowl",
    "brick",
    "clay_ball",
    "slime_ball",
    "flint",
    "string",
    "feather",
    "leather",
    "bone",
    "bone_meal",
    "coal",
    "charcoal",
    "diamond",
    "emerald",
    "lapis_lazuli",
    "redstone",
    "quartz",
    "raw_iron",
    "raw_gold",
    "raw_copper",
    "iron_ingot",
    "gold_ingot",
    "gold_nugget",
    "copper_ingot",
    "netherite_ingot",
    "netherite_scrap",
    "wheat",
    "wheat_seeds",
    "pumpkin_seeds",
    "melon_seeds",
    "beetroot_seeds",
    "paper",
    "sugar",
    "gunpowder",
    "apple",
    "golden_apple",
    "enchanted_golden_apple",
    "bread",
    "carrot",
    "golden_carrot",
    "potato",
    "baked_potato",
    "poisonous_potato",
    "beetroot",
    "melon_slice",
    "dried_kelp",
    "sweet_berries",
    "glow_berries",
    "cod",
    "cooked_cod",
    "salmon",
    "cooked_salmon",
    "tropical_fish",
    "pufferfish",
    "beef",
    "cooked_beef",
    "porkchop",
    "cooked_porkchop",
    "mutton",
    "cooked_mutton",
    "chicken",
    "cooked_chicken",
    "rabbit",
    "cooked_rabbit",
    "rotten_flesh",
    "spider_eye",
    "pumpkin_pie",
    "cookie",
    "chorus_fruit",
];

/// Vanilla defaults for ordinary items whose inventory behavior is implemented.
/// This does not mean their use, placement, tools, or rendering are complete.
pub fn ordinary_item_default_max(id: &str) -> Option<u32> {
    if id == "minecraft:shears" {
        return Some(1);
    }
    if crate::stair_items::is_stair_item(id) {
        return Some(64);
    }
    if crate::slab_mining::material(id).is_some() {
        return Some(64);
    }
    if native_core::button::Button::from_id(id).is_some() && id.starts_with("minecraft:") {
        return Some(64);
    }
    if let Some(max) = basic_item_max(id) {
        return Some(max);
    }
    let name = id.strip_prefix("minecraft:")?;
    if matches!(name, "ender_pearl" | "snowball") {
        return Some(16);
    }
    if STACKABLE_ITEMS.contains(&name) {
        return Some(64);
    }
    if TOOL_MATERIALS.iter().any(|material| {
        name.strip_prefix(material)
            .and_then(|suffix| suffix.strip_prefix('_'))
            .is_some_and(|suffix| TOOL_KINDS.contains(&suffix))
    }) {
        return Some(1);
    }
    let name = name.strip_prefix("stripped_").unwrap_or(name);
    for wood in WOODS {
        if let Some(suffix) = name.strip_prefix(wood).and_then(|s| s.strip_prefix('_'))
            && (matches!(suffix, "log" | "wood")
                || (suffix == "planks" && !id.contains(":stripped_")))
        {
            return Some(64);
        }
    }
    None
}

/// Default durability for the ordinary tool families implemented above.
pub fn ordinary_item_default_durability(id: &str) -> Option<u32> {
    if id == "minecraft:shears" {
        return Some(238);
    }
    if ordinary_item_default_max(id) != Some(1) {
        return None;
    }
    let (material, _) = id.strip_prefix("minecraft:")?.split_once('_')?;
    Some(match material {
        "wooden" => 59,
        "stone" => 131,
        "copper" => 190,
        "iron" => 250,
        "golden" => 32,
        "diamond" => 1561,
        "netherite" => 2031,
        _ => return None,
    })
}

/// Resolve a saved patch only when its effects on ordinary slot operations are
/// known. Other component patches remain intact in an unresolved stack.
pub fn saved_item_max(id: &str, components: &Tag) -> Option<u32> {
    let default = ordinary_item_default_max(id)?;
    if matches!(id, "minecraft:chest" | "minecraft:trapped_chest")
        && crate::saved_chest::placed_item_components(components).is_ok()
    {
        return Some(default);
    }
    let Tag::Compound(entries) = components else {
        return None;
    };
    let mut max = default;
    let mut seen = 0_u8;
    for (name, value) in entries {
        let flag = if name.matches("minecraft:max_stack_size") {
            max = value
                .as_java_int()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| (1..=99).contains(value))?;
            1
        } else if name.matches("minecraft:damage") && default == 1 {
            if !matches!(value, Tag::Int(damage) if *damage >= 0) {
                return None;
            }
            2
        } else if name.matches("minecraft:custom_data") {
            if !matches!(value, Tag::Compound(_)) {
                return None;
            }
            4
        } else if name.matches("minecraft:enchantment_glint_override") {
            if !matches!(value, Tag::Byte(0 | 1)) {
                return None;
            }
            8
        } else if name.matches("!minecraft:enchantment_glint_override") {
            if !matches!(value, Tag::Compound(fields) if fields.is_empty()) {
                return None;
            }
            16
        } else if name.matches("!minecraft:tool") {
            if !matches!(value, Tag::Compound(fields) if fields.is_empty()) {
                return None;
            }
            64
        } else if name.matches("minecraft:tool") {
            crate::custom_tool::Tool::parse(value)?;
            128
        } else if name.matches("minecraft:enchantments")
            && ordinary_item_default_durability(id).is_some()
        {
            let Tag::Compound(fields) = value else {
                return None;
            };
            let mut enchantments = 0_u8;
            for (name, level) in fields {
                let flag = if name.matches("minecraft:efficiency") {
                    1
                } else if name.matches("minecraft:unbreaking") {
                    2
                } else if name.matches("minecraft:silk_touch") {
                    4
                } else if name.matches("minecraft:fortune") {
                    8
                } else {
                    return None;
                };
                if !matches!(level, Tag::Int(1..=255)) || enchantments & flag != 0 {
                    return None;
                }
                enchantments |= flag;
            }
            32
        } else {
            return None;
        };
        if seen & flag != 0 {
            return None;
        }
        seen |= flag;
    }
    if seen & 24 == 24 {
        return None;
    }
    if seen & 192 == 192 {
        return None;
    }
    // Durable item prototypes require special stackability rules when their
    // maximum is changed; retain those patches until those rules exist.
    if default == 1 && max != 1 {
        return None;
    }
    Some(max)
}

/// The caller evaluates item- and player-specific slot rules before the click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotRules {
    pub max_count: u32,
    pub may_take: bool,
    pub accepts_carried: bool,
    pub accepts_existing: bool,
}

impl SlotRules {
    pub const STANDARD: Self = Self {
        max_count: 99,
        may_take: true,
        accepts_carried: true,
        accepts_existing: true,
    };
}

/// Performs a basic primary/secondary slot click. Returns whether stacks changed.
/// Does not run item-specific overrides, crafting effects, dropping, or callbacks.
pub fn pickup(
    slot: &mut Option<ItemStack>,
    cursor: &mut Option<ItemStack>,
    button: ClickButton,
    rules: SlotRules,
) -> bool {
    if slot.as_ref().is_some_and(|item| item.max_count.is_none())
        || cursor.as_ref().is_some_and(|item| item.max_count.is_none())
    {
        return false;
    }
    let mut current = slot.take();
    let mut held = cursor.take();
    let changed = match (&mut current, &mut held) {
        (None, Some(item)) if rules.accepts_carried => {
            let requested = if button == ClickButton::Primary {
                item.count
            } else {
                1
            };
            let amount = requested
                .min(item.count)
                .min(item.max_count.expect("resolved stack"))
                .min(rules.max_count);
            if amount == 0 {
                false
            } else {
                let mut placed = item.clone();
                placed.count = amount;
                item.count -= amount;
                current = Some(placed);
                true
            }
        }
        (Some(item), None) if rules.may_take => {
            let amount = if button == ClickButton::Primary {
                item.count
            } else {
                item.count.div_ceil(2)
            };
            let mut taken = item.clone();
            taken.count = amount;
            item.count -= amount;
            held = Some(taken);
            true
        }
        (Some(item), Some(carried)) if rules.may_take => {
            if rules.accepts_carried {
                if item.same_item_and_components(carried) {
                    let requested = if button == ClickButton::Primary {
                        carried.count
                    } else {
                        1
                    };
                    let room = carried
                        .max_count
                        .expect("resolved stack")
                        .min(rules.max_count)
                        .saturating_sub(item.count);
                    let amount = requested.min(carried.count).min(room);
                    item.count += amount;
                    carried.count -= amount;
                    amount != 0
                } else if carried.count
                    <= carried
                        .max_count
                        .expect("resolved stack")
                        .min(rules.max_count)
                {
                    std::mem::swap(item, carried);
                    true
                } else {
                    false
                }
            } else if item.same_item_and_components(carried) {
                let room = carried
                    .max_count
                    .expect("resolved stack")
                    .saturating_sub(carried.count);
                if !rules.accepts_existing && room < item.count {
                    false
                } else {
                    let amount = item.count.min(room);
                    item.count -= amount;
                    carried.count += amount;
                    amount != 0
                }
            } else {
                false
            }
        }
        _ => false,
    };
    *slot = current.filter(|item| item.count != 0);
    *cursor = held.filter(|item| item.count != 0);
    changed
}

fn max_component_consistent(item: &ItemStack) -> bool {
    item.components
        .get("minecraft:max_stack_size")
        .is_none_or(|value| {
            value
                .as_java_int()
                .and_then(|value| u32::try_from(value).ok())
                == item.max_count
        })
}

fn equal_compounds(a: &[(JavaString, Tag)], b: &[(JavaString, Tag)], ignore_max: bool) -> bool {
    fn unique(entries: &[(JavaString, Tag)]) -> impl Iterator<Item = &(JavaString, Tag)> {
        entries.iter().enumerate().filter_map(|(i, entry)| {
            (!entries[i + 1..].iter().any(|(key, _)| key == &entry.0)).then_some(entry)
        })
    }
    let relevant =
        |entry: &&(JavaString, Tag)| !ignore_max || !entry.0.matches("minecraft:max_stack_size");
    let mut count = 0;
    for (key, value) in unique(a).filter(relevant) {
        count += 1;
        let Some((_, other)) = unique(b).filter(relevant).find(|(name, _)| name == key) else {
            return false;
        };
        if !equal_tag(value, other) {
            return false;
        }
    }
    count == unique(b).filter(relevant).count()
}

// NBT compounds are maps in Java: field order is irrelevant and a duplicate key
// resolves to the last value. Preserve list order and Java's float bit semantics.
pub(crate) fn equal_tag(a: &Tag, b: &Tag) -> bool {
    match (a, b) {
        (Tag::Compound(a), Tag::Compound(b)) => equal_compounds(a, b, false),
        (
            Tag::List {
                element_type: a_type,
                values: a,
            },
            Tag::List {
                element_type: b_type,
                values: b,
            },
        ) => {
            a_type == b_type && a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal_tag(a, b))
        }
        (Tag::Float(a), Tag::Float(b)) => (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits(),
        (Tag::Double(a), Tag::Double(b)) => {
            (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn efficiency_tool_retains_enchantment_when_damaged() {
        struct Data;
        impl native_resources::packs::ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, path: &str) -> anyhow::Result<Vec<ResourceId>> {
                Ok(if path == "enchantment" {
                    vec![ResourceId::parse("minecraft:efficiency")?]
                } else {
                    vec![]
                })
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(path
                    .ends_with("/enchantment/efficiency.json")
                    .then(|| br#"{"slots":["mainhand"],"effects":{}}"#.to_vec()))
            }
        }
        let components = Tag::Compound(vec![(
            "minecraft:enchantments".into(),
            Tag::Compound(vec![("minecraft:efficiency".into(), Tag::Int(5))]),
        )]);
        assert_eq!(
            saved_item_max("minecraft:iron_pickaxe", &components),
            Some(1)
        );
        let tool =
            ItemStack::new("minecraft:iron_pickaxe".into(), 1, 1, components.clone()).unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_enchantments(Arc::new(OxygenEnchantments::load(&mut Data).unwrap()));
        inventory.set_slot(PlayerSlot::Storage(0), Some(tool));
        assert!(inventory.damage_selected_tool(1, &mut LegacyRandom::new(0)));
        let after = inventory.slot(PlayerSlot::Storage(0)).unwrap();
        assert_eq!(
            after.components().get("minecraft:enchantments"),
            components.get("minecraft:enchantments")
        );
        assert_eq!(
            after.components().get("minecraft:damage"),
            Some(&Tag::Int(1))
        );
        let unbreaking = Tag::Compound(vec![(
            "minecraft:enchantments".into(),
            Tag::Compound(vec![("minecraft:unbreaking".into(), Tag::Int(1))]),
        )]);
        assert_eq!(
            saved_item_max("minecraft:iron_pickaxe", &unbreaking),
            Some(1)
        );
    }

    #[test]
    fn tool_damage_uses_active_pack_effects_and_world_random() {
        struct Data(f32);
        impl native_resources::packs::ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, path: &str) -> anyhow::Result<Vec<ResourceId>> {
                Ok(if path == "enchantment" {
                    vec![
                        ResourceId::parse("minecraft:efficiency")?,
                        ResourceId::parse("minecraft:unbreaking")?,
                    ]
                } else {
                    vec![]
                })
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(if path.ends_with("/enchantment/efficiency.json") {
                    Some(br#"{"slots":["mainhand"],"effects":{}}"#.to_vec())
                } else if path.ends_with("/enchantment/unbreaking.json") {
                    Some(format!(r#"{{"slots":["any"],"effects":{{"minecraft:item_damage":[{{"effect":{{"type":"minecraft:remove_binomial","chance":{}}}}}]}}}}"#, self.0).into_bytes())
                } else {
                    None
                })
            }
        }
        for chance in [0.0, 1.0] {
            let components = Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![
                    ("minecraft:efficiency".into(), Tag::Int(5)),
                    ("minecraft:unbreaking".into(), Tag::Int(3)),
                ]),
            )]);
            assert_eq!(
                saved_item_max("minecraft:iron_pickaxe", &components),
                Some(1)
            );
            let tool =
                ItemStack::new("minecraft:iron_pickaxe".into(), 1, 1, components.clone()).unwrap();
            let mut inventory = PlayerInventory::default();
            inventory.set_enchantments(Arc::new(
                OxygenEnchantments::load(&mut Data(chance)).unwrap(),
            ));
            inventory.set_slot(PlayerSlot::Storage(0), Some(tool));
            let mut random = LegacyRandom::new(123);
            let mut expected = random.clone();
            expected.next_float();
            assert_eq!(
                inventory.damage_selected_tool(1, &mut random),
                chance == 0.0
            );
            assert_eq!(random.next_int(), expected.next_int());
            let item = inventory.slot(PlayerSlot::Storage(0)).unwrap();
            assert_eq!(
                item.components().get("minecraft:enchantments"),
                components.get("minecraft:enchantments")
            );
            assert_eq!(
                item.components().get("minecraft:damage"),
                if chance == 0.0 {
                    Some(&Tag::Int(1))
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn pick_item_preserves_enchanted_slots_and_unresolved_items() {
        let stack = |id: &str, components| ItemStack::new(id.into(), 1, 64, components).unwrap();
        let plain = || Tag::Compound(vec![]);
        let enchanted = || {
            Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![("minecraft:unbreaking".into(), Tag::Int(1))]),
            )])
        };
        let mut inventory = PlayerInventory::default();
        for i in 0..9 {
            inventory.set_slot(
                PlayerSlot::Storage(i),
                Some(stack("minecraft:dirt", enchanted())),
            );
        }
        inventory.set_slot(
            PlayerSlot::Storage(5),
            Some(stack("minecraft:dirt", plain())),
        );
        inventory.select(3);
        assert!(inventory.pick_item(stack("minecraft:stone", plain()), true));
        assert_eq!(inventory.selected(), 5);
        assert_eq!(
            inventory.slot(PlayerSlot::Storage(9)).unwrap().id(),
            "minecraft:dirt"
        );
        assert_eq!(
            inventory.slot(PlayerSlot::Storage(5)).unwrap().id(),
            "minecraft:stone"
        );

        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::unresolved("minecraft:stone".into(), 1, plain()),
        );
        let before = inventory.clone();
        assert!(!inventory.pick_item(stack("minecraft:stone", plain()), true));
        assert_eq!(inventory, before);
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn death_inventory_and_container_return_match_java() {
        use serde_json::{Value, json};
        use std::{ffi::OsString, path::PathBuf, process::Command};
        const IDS: [&str; 9] = [
            "",
            "minecraft:stone",
            "minecraft:dirt",
            "minecraft:apple",
            "minecraft:oak_planks",
            "minecraft:cobblestone",
            "minecraft:grass_block",
            "minecraft:iron_ingot",
            "minecraft:gold_ingot",
        ];
        fn next(seed: &mut u64) -> u32 {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (*seed >> 32) as u32
        }
        fn generate(seed: &mut u64, filled: u32, full_stack: bool, allow_curse: bool) -> Value {
            if next(seed) % 100 >= filled {
                return json!([0, 0, 0]);
            }
            let id = next(seed) % 8 + 1;
            let count = if full_stack { 64 } else { next(seed) % 64 + 1 };
            let cursed = allow_curse && next(seed).is_multiple_of(4);
            json!([id, count, u8::from(cursed)])
        }
        fn stack_from(value: &Value) -> Option<ItemStack> {
            let id = value[0].as_u64().unwrap() as usize;
            if id == 0 {
                return None;
            }
            let components = if value[2].as_u64() == Some(1) {
                Tag::Compound(vec![(
                    "minecraft:enchantments".into(),
                    Tag::Compound(vec![("minecraft:vanishing_curse".into(), Tag::Int(1))]),
                )])
            } else {
                Tag::Compound(vec![])
            };
            ItemStack::new(
                IDS[id].into(),
                value[1].as_u64().unwrap() as u32,
                64,
                components,
            )
        }
        fn describe(item: Option<&ItemStack>) -> Value {
            item.map_or(json!([0, 0, 0]), |item| {
                let id = IDS.iter().position(|id| *id == item.id()).unwrap();
                let cursed = item
                    .components()
                    .get("minecraft:enchantments")
                    .and_then(|value| value.get("minecraft:vanishing_curse"))
                    .is_some();
                json!([id, item.count(), u8::from(cursed)])
            })
        }
        let mut seed = 7_u64;
        let mut cases = Vec::new();
        for index in 0..256 {
            let density = [0, 50, 95, 100][index % 4];
            let full = index % 8 == 3;
            let slots: Vec<_> = (0..36)
                .map(|_| generate(&mut seed, density, full, true))
                .collect();
            let armor: Vec<_> = (0..4)
                .map(|_| generate(&mut seed, 65, false, true))
                .collect();
            let craft: Vec<_> = (0..4)
                .map(|_| generate(&mut seed, 70, false, false))
                .collect();
            let cursor = generate(&mut seed, 75, false, false);
            let offhand = generate(&mut seed, 70, false, true);
            cases.push(
                json!({"keep":(index / 4) % 2 == 0, "selected":next(&mut seed)%9,
                "slots":slots,"armor":armor,"craft":craft,"cursor":cursor,"offhand":offhand}),
            );
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let enchantments = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
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
        let classpath: OsString = std::env::join_paths(paths).unwrap();
        let java_bin = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("DeathInventoryOracle.java");
        let input = temp.path().join("cases.json");
        let output = temp.path().join("results.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/DeathInventoryOracle.java"),
        )
        .unwrap();
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        let compiled = Command::new(java_bin.join("javac"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let run = Command::new(java_bin.join("java"))
            .arg("-cp")
            .arg(&classpath)
            .arg("DeathInventoryOracle")
            .arg(&input)
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(cases.len(), results.len());
        for (case, expected) in cases.iter().zip(results) {
            let mut inventory = PlayerInventory {
                menu: Some(InventoryMenu::Player),
                selected: case["selected"].as_u64().unwrap() as u8,
                cursor: stack_from(&case["cursor"]),
                offhand: stack_from(&case["offhand"]),
                enchantments: Some(Arc::clone(&enchantments)),
                ..Default::default()
            };
            for i in 0..36 {
                inventory.storage[i] = stack_from(&case["slots"][i]);
            }
            for i in 0..4 {
                inventory.armor[i] = stack_from(&case["armor"][i]);
                inventory.crafting[i] = stack_from(&case["craft"][i]);
            }
            let drops = inventory
                .take_death_drops(case["keep"].as_bool().unwrap())
                .unwrap();
            let actual = json!({
                "loot":drops.inventory.iter().map(|item| describe(Some(item))).collect::<Vec<_>>(),
                "overflow":drops.container_overflow.iter().map(|item| describe(Some(item))).collect::<Vec<_>>(),
                "slots":inventory.storage.iter().map(|item| describe(item.as_ref())).collect::<Vec<_>>(),
                "armor":inventory.armor.iter().map(|item| describe(item.as_ref())).collect::<Vec<_>>(),
                "offhand":describe(inventory.offhand.as_ref()),
            });
            assert_eq!(actual, expected, "case {case}");
        }
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn experience_repair_matches_vanilla_orb_and_player_random() {
        use native_resources::packs::ResourceStack;
        use serde_json::{Value, json};
        use std::{ffi::OsString, path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let enchantments = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
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
        let classpath: OsString = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("ExperienceRepairOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/ExperienceRepairOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "repair oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source));
        let slots = [
            PlayerSlot::Storage(0),
            PlayerSlot::Offhand,
            PlayerSlot::Armor(3),
            PlayerSlot::Armor(2),
            PlayerSlot::Armor(1),
            PlayerSlot::Armor(0),
        ];
        let types = [
            "minecraft:diamond_pickaxe",
            "minecraft:diamond_sword",
            "minecraft:diamond_boots",
            "minecraft:diamond_leggings",
            "minecraft:diamond_chestplate",
            "minecraft:diamond_helmet",
        ];
        let mut cases = Vec::new();
        let mut outputs = Vec::new();
        for index in 0..256 {
            let amount = [1, 3, 7, 17, 37, 73, 149, 2477][index % 8];
            let seed = index as i64 * 71 + 13;
            let mut generator = LegacyRandom::new(index as i64 * 193 + 29);
            let mut inventory = PlayerInventory::default();
            inventory.set_enchantments(Arc::clone(&enchantments));
            let mut damages = Vec::new();
            let mut flags = Vec::new();
            for i in 0..6 {
                let damage =
                    [0, 1, 2, 3, 5, 17, 48, 201][generator.next_int_bound(8).unwrap() as usize];
                let mending = generator.next_bool();
                let mut components = vec![("minecraft:damage".into(), Tag::Int(damage))];
                if mending {
                    components.push((
                        "minecraft:enchantments".into(),
                        Tag::Compound(vec![("minecraft:mending".into(), Tag::Int(1))]),
                    ));
                }
                inventory.set_slot(
                    slots[i],
                    ItemStack::new(types[i].into(), 1, 1, Tag::Compound(components)),
                );
                damages.push(damage);
                flags.push(mending);
            }
            let mut rng = LegacyRandom::new(seed);
            let (remaining, after) = inventory.repair_with_experience(amount, &mut rng).unwrap();
            let after = after.as_ref().unwrap_or(&inventory);
            let result_damage: Vec<_> = slots
                .iter()
                .map(|slot| {
                    after
                        .slot(*slot)
                        .unwrap()
                        .components()
                        .get("minecraft:damage")
                        .unwrap()
                        .as_java_int()
                        .unwrap()
                })
                .collect();
            cases.push(json!({"seed":seed,"xp":amount,"damage":damages,"mending":flags}));
            outputs.push(
                json!({"remaining":remaining,"damage":result_damage,"next_random":rng.next_int()}),
            );
        }
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        let saved = temp.path().join("saved-item.nbt");
        let effect_input = temp.path().join("effect-input.json");
        let effect_output = temp.path().join("effect-output.json");
        let effect_key_output = temp.path().join("effect-key-output.json");
        struct EffectSource(Value);
        impl native_resources::packs::ResourceProvider for EffectSource {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, folder: &str) -> anyhow::Result<Vec<ResourceId>> {
                if folder == "enchantment" {
                    Ok(vec![ResourceId::parse("example:repair")?])
                } else {
                    Ok(Vec::new())
                }
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                if path == "data/example/enchantment/repair.json" {
                    Ok(Some(serde_json::to_vec(
                        &json!({"slots":["any"],"effects":{"minecraft:repair_with_xp":[{"effect":self.0}]}}),
                    )?))
                } else {
                    Ok(None)
                }
            }
        }
        let effects = [
            json!({"type":"minecraft:add","value":{"type":"minecraft:linear","base":1.25,"per_level_above_first":0.5}}),
            json!({"type":"minecraft:multiply","factor":2.0}),
            json!({"type":"minecraft:set","value":3.75}),
            json!({"type":"minecraft:exponential","base":2.0,"exponent":3.0}),
            json!({"type":"minecraft:all_of","effects":[{"type":"minecraft:add","value":0.5},{"type":"minecraft:multiply","factor":1.5}]}),
            json!({"type":":multiply","factor":2.0}),
            json!({"type":":add","value":{"type":":linear","base":1.25,"per_level_above_first":0.5}}),
        ];
        let mut effect_cases = Vec::new();
        let mut effect_expected = Vec::new();
        for effect in effects {
            let registry = OxygenEnchantments::load(&mut EffectSource(effect.clone())).unwrap();
            for level in [1, 2, 5, 10] {
                for value in [0.0_f32, 1.0, 7.0, 17.5, 2477.0] {
                    effect_cases.push(json!({"effect":effect,"level":level,"value":value}));
                    effect_expected.push(
                        registry
                            .apply_repair_with_xp(
                                &ResourceId::parse("example:repair").unwrap(),
                                level,
                                value,
                            )
                            .unwrap()
                            .expect("test effect is present")
                            .to_bits() as i32,
                    );
                }
            }
        }
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        std::fs::write(&effect_input, serde_json::to_vec(&effect_cases).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("ExperienceRepairOracle")
            .arg(&input)
            .arg(&output)
            .arg(&saved)
            .arg(&effect_input)
            .arg(&effect_output)
            .arg(&effect_key_output));
        let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(results.len(), cases.len());
        for (index, (actual, expected)) in outputs.iter().zip(results.iter()).enumerate() {
            assert_eq!(actual, expected, "case {index}: {}", cases[index]);
        }
        let effect_results: Vec<i32> =
            serde_json::from_slice(&std::fs::read(effect_output).unwrap()).unwrap();
        assert_eq!(effect_results, effect_expected);
        let key_results: Vec<bool> =
            serde_json::from_slice(&std::fs::read(effect_key_output).unwrap()).unwrap();
        assert_eq!(key_results, [true, true, true, false, false, false]);
        let mut item = native_persistence::nbt::read(&std::fs::read(saved).unwrap())
            .unwrap()
            .value;
        let Tag::Compound(fields) = &mut item else {
            panic!("Java saved item is not a compound")
        };
        fields.push(("Slot".into(), Tag::Byte(0)));
        let player = native_persistence::nbt::NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Inventory".into(),
                Tag::List {
                    element_type: native_persistence::nbt::TagType::Compound,
                    values: vec![item],
                },
            )]),
        };
        let mut inventory = crate::player_data::read_saved_player(&player).unwrap();
        assert_eq!(
            inventory.slot(PlayerSlot::Storage(0)).unwrap().max_count(),
            Some(1)
        );
        inventory.set_enchantments(enchantments);
        let (remaining, repaired) = inventory
            .repair_with_experience(7, &mut LegacyRandom::new(9))
            .unwrap();
        assert_eq!(remaining, 0);
        assert_eq!(
            repaired
                .unwrap()
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .components()
                .get("minecraft:damage"),
            Some(&Tag::Int(7))
        );
    }

    #[test]
    fn experience_pickup_resolves_repair_effect_and_equipment_slot_from_data_pack() {
        use native_resources::packs::ResourceProvider;
        struct Source {
            repair: bool,
            group: &'static str,
            abbreviation: u8,
        }
        impl ResourceProvider for Source {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, _: &str) -> anyhow::Result<Vec<ResourceId>> {
                Ok(vec![ResourceId::parse("example:enchantment")?])
            }
            fn read_data(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                let effects = if self.repair && self.abbreviation == 2 {
                    serde_json::json!({":repair_with_xp":[{"effect":{"type":":multiply","factor":2}}]})
                } else if self.repair && self.abbreviation == 1 {
                    serde_json::json!({"repair_with_xp":[{"effect":{"type":"multiply","factor":2}}]})
                } else if self.repair {
                    serde_json::json!({"minecraft:repair_with_xp":[{"effect":{"type":"minecraft:multiply","factor":2}}]})
                } else {
                    serde_json::json!({})
                };
                Ok(Some(serde_json::to_vec(
                    &serde_json::json!({"slots":[self.group],"effects":effects}),
                )?))
            }
        }
        for (repair, group, abbreviation, remaining) in [
            (false, "hand", 0, 7),
            (true, "armor", 0, 7),
            (true, "hand", 0, 3),
            (true, "hand", 1, 3),
            (true, "hand", 2, 3),
        ] {
            let stack = ItemStack::new(
                "minecraft:diamond_pickaxe".into(),
                1,
                1,
                Tag::Compound(vec![
                    ("minecraft:damage".into(), Tag::Int(8)),
                    (
                        "minecraft:enchantments".into(),
                        Tag::Compound(vec![("example:enchantment".into(), Tag::Int(1))]),
                    ),
                ]),
            )
            .unwrap();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(PlayerSlot::Storage(0), Some(stack));
            assert!(
                inventory
                    .repair_with_experience(7, &mut LegacyRandom::new(1))
                    .is_err()
            );
            inventory.set_enchantments(Arc::new(
                OxygenEnchantments::load(&mut Source {
                    repair,
                    group,
                    abbreviation,
                })
                .unwrap(),
            ));
            let (result, repaired) = inventory
                .repair_with_experience(7, &mut LegacyRandom::new(1))
                .unwrap();
            assert_eq!(result, remaining);
            if repair && group == "hand" {
                let repaired = repaired.unwrap();
                assert_eq!(
                    repaired
                        .slot(PlayerSlot::Storage(0))
                        .unwrap()
                        .components()
                        .get("minecraft:damage"),
                    Some(&Tag::Int(0))
                );
            } else {
                assert!(repaired.is_none());
            }
        }
    }

    #[test]
    fn multiple_repair_enchantments_are_withheld_until_their_order_is_known() {
        use native_resources::packs::ResourceProvider;
        struct Source;
        impl ResourceProvider for Source {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, folder: &str) -> anyhow::Result<Vec<ResourceId>> {
                if folder == "enchantment" {
                    ["example:first", "example:second", "example:other"]
                        .into_iter()
                        .map(ResourceId::parse)
                        .collect()
                } else {
                    Ok(Vec::new())
                }
            }
            fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                let data = match path {
                    "data/example/enchantment/first.json" => br#"{"slots":["any"],"effects":{"minecraft:repair_with_xp":[{"effect":{"type":"minecraft:multiply","factor":2}}]}}"#.as_slice(),
                    "data/example/enchantment/second.json" => br#"{"slots":["any"],"effects":{"minecraft:repair_with_xp":[{"effect":{"type":"minecraft:add","value":3}}]}}"#.as_slice(),
                    "data/example/enchantment/other.json" => br#"{"slots":["any"],"effects":{}}"#.as_slice(),
                    _ => return Ok(None),
                };
                Ok(Some(data.to_vec()))
            }
        }

        let registry = Arc::new(OxygenEnchantments::load(&mut Source).unwrap());
        for (names, supported) in [
            (["example:first", "example:other"], true),
            (["example:first", "example:second"], false),
            (["example:second", "example:first"], false),
        ] {
            let stack = ItemStack::new(
                "minecraft:diamond_pickaxe".into(),
                1,
                1,
                Tag::Compound(vec![
                    ("minecraft:damage".into(), Tag::Int(20)),
                    (
                        "minecraft:enchantments".into(),
                        Tag::Compound(
                            names
                                .into_iter()
                                .map(|name| (name.into(), Tag::Int(1)))
                                .collect(),
                        ),
                    ),
                ]),
            )
            .unwrap();
            let mut inventory = PlayerInventory::default();
            inventory.set_enchantments(Arc::clone(&registry));
            inventory.set_slot(PlayerSlot::Storage(0), Some(stack));
            let result = inventory.repair_with_experience(7, &mut LegacyRandom::new(1));
            if supported {
                let (remaining, repaired) = result.unwrap();
                assert_eq!(remaining, 0);
                assert_eq!(
                    repaired
                        .unwrap()
                        .slot(PlayerSlot::Storage(0))
                        .unwrap()
                        .components()
                        .get("minecraft:damage"),
                    Some(&Tag::Int(6))
                );
            } else {
                assert!(result.is_err());
                assert_eq!(
                    inventory
                        .slot(PlayerSlot::Storage(0))
                        .unwrap()
                        .components()
                        .get("minecraft:damage"),
                    Some(&Tag::Int(20))
                );
            }
        }
    }

    #[test]
    fn death_drops_honor_pack_curses_and_are_transactional_when_unknown() {
        use native_resources::packs::ResourceProvider;
        struct Source;
        impl ResourceProvider for Source {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, _: &str) -> anyhow::Result<Vec<ResourceId>> {
                Ok(vec![ResourceId::parse("example:vanishes")?])
            }
            fn read_data(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(Some(
                    br#"{"slots":["any"],"effects":{"minecraft:prevent_equipment_drop":{}}}"#
                        .to_vec(),
                ))
            }
        }
        let components = Tag::Compound(vec![(
            "minecraft:enchantments".into(),
            Tag::Compound(vec![("example:vanishes".into(), Tag::Int(1))]),
        )]);
        let cursed = ItemStack::new("minecraft:stone".into(), 1, 64, components).unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(PlayerSlot::Storage(0), Some(cursed.clone()));
        inventory.set_slot(PlayerSlot::Storage(1), stack("minecraft:dirt", 2));
        let before = inventory.clone();
        assert!(inventory.take_death_drops(false).is_err());
        assert_eq!(inventory, before);
        inventory.set_enchantments(Arc::new(OxygenEnchantments::load(&mut Source).unwrap()));
        let drops = inventory.take_death_drops(false).unwrap();
        assert_eq!(drops.inventory.len(), 1);
        assert_eq!(drops.inventory[0].id(), "minecraft:dirt");
        assert!(drops.container_overflow.is_empty());
        assert!(inventory.slot(PlayerSlot::Storage(0)).is_none());

        let mut keep = PlayerInventory::default();
        keep.set_slot(PlayerSlot::Storage(0), Some(cursed));
        let kept = keep.take_death_drops(true).unwrap();
        assert!(kept.inventory.is_empty());
        assert!(kept.container_overflow.is_empty());
        assert!(keep.slot(PlayerSlot::Storage(0)).is_some());
    }

    #[test]
    fn equipment_removal_obeys_active_data_pack_effect_and_creative_mode() {
        use native_resources::packs::ResourceProvider;
        struct Data(bool);
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, _: &str) -> anyhow::Result<Vec<ResourceId>> {
                Ok(vec![ResourceId::parse("example:custom_curse")?])
            }
            fn read_data(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
                let effects = if self.0 {
                    serde_json::json!({"minecraft:prevent_armor_change":{}})
                } else {
                    serde_json::json!({})
                };
                Ok(Some(serde_json::to_vec(
                    &serde_json::json!({"slots":["armor"],"effects":effects}),
                )?))
            }
        }
        let components = Tag::Compound(vec![(
            "minecraft:enchantments".into(),
            Tag::Compound(vec![("example:custom_curse".into(), Tag::Int(1))]),
        )]);
        let item = ItemStack::new("minecraft:iron_helmet".into(), 1, 1, components).unwrap();
        for (prevent, creative, expected) in [
            (true, false, false),
            (true, true, true),
            (false, false, true),
        ] {
            let mut inventory = PlayerInventory::default();
            inventory.set_enchantments(Arc::new(
                OxygenEnchantments::load(&mut Data(prevent)).unwrap(),
            ));
            inventory.set_slot(PlayerSlot::Armor(0), Some(item.clone()));
            assert!(inventory.toggle());
            assert_eq!(
                inventory.click_with_mode(PlayerSlot::Armor(0), ClickButton::Primary, creative),
                expected
            );
            assert_eq!(inventory.cursor().is_some(), expected);
        }
    }

    fn stack(id: &str, count: u32) -> Option<ItemStack> {
        ItemStack::new(id.into(), count, 64, Tag::Compound(vec![]))
    }

    #[test]
    fn chest_clicks_and_close_preserve_contents_until_returned() {
        let mut inventory = PlayerInventory::default();
        let mut chest = std::array::from_fn(|_| None);
        chest[0] = stack("minecraft:stone", 8);
        let position = IVec3::new(2, 64, -3);
        assert!(inventory.open_chest(position, chest));
        assert!(inventory.click(PlayerSlot::Chest(0), ClickButton::Secondary));
        assert_eq!(inventory.cursor().unwrap().count(), 4);
        assert!(inventory.click(PlayerSlot::Storage(3), ClickButton::Primary));
        assert_eq!(inventory.slot(PlayerSlot::Storage(3)).unwrap().count(), 4);
        assert!(inventory.toggle());
        assert!(!inventory.ready_for_player_save());
        assert!(!inventory.open_chest(position, std::array::from_fn(|_| None)));
        assert!(!inventory.toggle());
        let (returned_position, other, slots) = inventory.take_closed_chest().unwrap();
        assert_eq!(returned_position, position);
        assert_eq!(other, None);
        assert_eq!(slots[0].as_ref().unwrap().count(), 4);
        assert!(inventory.ready_for_player_save());
    }

    #[test]
    fn max_stack_patch_uses_effective_component_equality() {
        let default =
            ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        let explicit = ItemStack::new(
            "minecraft:stone".into(),
            1,
            64,
            Tag::Compound(vec![("minecraft:max_stack_size".into(), Tag::Int(64))]),
        )
        .unwrap();
        assert!(default.same_item_and_components(&explicit));
        let inconsistent = ItemStack::new(
            "minecraft:stone".into(),
            1,
            64,
            Tag::Compound(vec![("minecraft:max_stack_size".into(), Tag::Int(99))]),
        )
        .unwrap();
        assert!(!default.same_item_and_components(&inconsistent));
        let custom = ItemStack::new(
            "minecraft:stone".into(),
            98,
            99,
            Tag::Compound(vec![("minecraft:max_stack_size".into(), Tag::Int(99))]),
        )
        .unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(PlayerSlot::Storage(0), Some(custom.clone()));
        let mut dropped = Some(
            ItemStack::new("minecraft:stone".into(), 1, 99, custom.components.clone()).unwrap(),
        );
        assert!(inventory.pickup_stack(&mut dropped, false));
        assert_eq!(inventory.slot(PlayerSlot::Storage(0)).unwrap().count(), 99);
        assert!(dropped.is_none());
    }

    #[test]
    fn primary_and_secondary_move_merge_and_swap_without_losing_items() {
        let mut slot = stack("minecraft:stone", 5);
        let mut cursor = None;
        assert!(pickup(
            &mut slot,
            &mut cursor,
            ClickButton::Secondary,
            SlotRules::STANDARD
        ));
        assert_eq!(slot.as_ref().unwrap().count(), 2);
        assert_eq!(cursor.as_ref().unwrap().count(), 3);
        assert!(pickup(
            &mut slot,
            &mut cursor,
            ClickButton::Secondary,
            SlotRules::STANDARD
        ));
        assert_eq!(slot.as_ref().unwrap().count(), 3);
        assert_eq!(cursor.as_ref().unwrap().count(), 2);
        cursor = stack("minecraft:dirt", 10);
        assert!(pickup(
            &mut slot,
            &mut cursor,
            ClickButton::Primary,
            SlotRules::STANDARD
        ));
        assert_eq!(slot.as_ref().unwrap().id(), "minecraft:dirt");
        assert_eq!(cursor.as_ref().unwrap().count(), 3);
    }

    #[test]
    fn component_maps_compare_by_content_and_nested_compound_order() {
        let a = Tag::Compound(vec![
            ("a".into(), Tag::Int(1)),
            (
                "b".into(),
                Tag::Compound(vec![("x".into(), Tag::Int(2)), ("y".into(), Tag::Int(3))]),
            ),
        ]);
        let b = Tag::Compound(vec![
            (
                "b".into(),
                Tag::Compound(vec![("y".into(), Tag::Int(3)), ("x".into(), Tag::Int(2))]),
            ),
            ("a".into(), Tag::Int(1)),
        ]);
        let a = ItemStack::new("minecraft:stone".into(), 1, 64, a).unwrap();
        let b = ItemStack::new("minecraft:stone".into(), 2, 64, b).unwrap();
        assert!(a.same_item_and_components(&b));
    }

    #[test]
    fn closing_returns_cursor_and_crafting_inputs_to_storage() {
        let mut inventory = PlayerInventory::demo();
        assert!(inventory.toggle());
        assert!(inventory.click(PlayerSlot::Storage(0), ClickButton::Secondary));
        assert!(inventory.click(PlayerSlot::Crafting(0), ClickButton::Primary));
        assert_eq!(inventory.slot(PlayerSlot::Crafting(0)).unwrap().count(), 32);
        assert!(inventory.toggle());
        assert!(!inventory.is_open());
        assert!(inventory.cursor().is_none());
        assert!(inventory.slot(PlayerSlot::Crafting(0)).is_none());
        assert_eq!(inventory.slot(PlayerSlot::Storage(0)).unwrap().count(), 64);
    }

    #[test]
    fn closing_with_no_room_preserves_every_item_until_drops_are_supported() {
        let mut inventory = PlayerInventory::default();
        for index in 0..36 {
            inventory.set_slot(PlayerSlot::Storage(index), stack("minecraft:stone", 64));
        }
        inventory.set_slot(PlayerSlot::Crafting(0), stack("minecraft:dirt", 1));
        assert!(inventory.toggle());
        let before = inventory.clone();
        assert!(!inventory.toggle());
        assert_eq!(inventory, before);
        assert!(inventory.is_open());
    }

    #[test]
    fn unresolved_items_remain_untouched_by_pickup() {
        let mut inventory = PlayerInventory::default();
        let item =
            ItemStack::unresolved("minecraft:bundle".into(), 1, Tag::Compound(vec![])).unwrap();
        inventory.set_slot(PlayerSlot::Storage(0), Some(item.clone()));
        assert!(inventory.toggle());
        assert!(!inventory.click(PlayerSlot::Storage(0), ClickButton::Primary));
        assert_eq!(inventory.slot(PlayerSlot::Storage(0)), Some(&item));
        assert!(inventory.cursor().is_none());
    }

    #[test]
    fn ordinary_sword_and_food_can_move_without_losing_component_data() {
        let patch = Tag::Compound(vec![
            ("minecraft:damage".into(), Tag::Int(23)),
            (
                "minecraft:custom_data".into(),
                Tag::Compound(vec![("owner".into(), Tag::Int(7))]),
            ),
        ]);
        let sword = ItemStack::new("minecraft:diamond_sword".into(), 1, 1, patch.clone()).unwrap();
        let apples =
            ItemStack::new("minecraft:apple".into(), 10, 64, Tag::Compound(vec![])).unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(PlayerSlot::Storage(0), Some(sword));
        inventory.set_slot(PlayerSlot::Storage(9), Some(apples));
        assert!(inventory.toggle());
        assert!(inventory.click(PlayerSlot::Storage(0), ClickButton::Primary));
        assert_eq!(inventory.cursor().unwrap().components(), &patch);
        assert!(inventory.click(PlayerSlot::Storage(1), ClickButton::Secondary));
        assert_eq!(
            inventory.slot(PlayerSlot::Storage(1)).unwrap().components(),
            &patch
        );
        assert!(inventory.click(PlayerSlot::Storage(9), ClickButton::Secondary));
        assert_eq!(inventory.cursor().unwrap().count(), 5);
        assert!(inventory.toggle());
        assert_eq!(inventory.slot(PlayerSlot::Storage(9)).unwrap().count(), 10);
        assert!(inventory.select(1));
        let mut thrown = inventory.drop_selected(false);
        assert_eq!(thrown.as_ref().unwrap().components(), &patch);
        assert!(inventory.pickup_stack(&mut thrown, false));
        assert!(thrown.is_none());
        assert_eq!(
            inventory.slot(PlayerSlot::Storage(0)).unwrap().components(),
            &patch
        );
    }

    #[test]
    fn unknown_or_inventory_affecting_patches_stay_unresolved() {
        let damage = Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(42))]);
        assert_eq!(saved_item_max("minecraft:diamond_sword", &damage), Some(1));
        assert_eq!(saved_item_max("minecraft:stone", &damage), None);
        let altered_tool = Tag::Compound(vec![
            ("minecraft:damage".into(), Tag::Int(42)),
            ("minecraft:max_stack_size".into(), Tag::Int(16)),
        ]);
        assert_eq!(
            saved_item_max("minecraft:diamond_sword", &altered_tool),
            None
        );
        let equipment = Tag::Compound(vec![("minecraft:equippable".into(), Tag::Compound(vec![]))]);
        assert_eq!(saved_item_max("minecraft:apple", &equipment), None);
        assert_eq!(
            saved_item_max("minecraft:bundle", &Tag::Compound(vec![])),
            None
        );
        assert_eq!(
            ordinary_item_default_max("minecraft:stripped_oak_planks"),
            None
        );
        assert_eq!(ordinary_item_default_max("other:diamond_sword"), None);
    }
}
