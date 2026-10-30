//! Minecraft's crafting recipe-book panel and its client-side state.

use std::{collections::BTreeMap, sync::Arc};

use native_persistence::nbt::Tag;
use native_resources::recipes::{CraftingCategory, CraftingRecipes, RecipeBookInventory};
use native_session::inventory::{
    ClickButton, InventoryMenu, ItemStack, PlayerInventory, ordinary_item_default_max,
};

use crate::{
    game_ui::stack_icon,
    held_items::ItemModelCache,
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Search,
    Equipment,
    Building,
    Misc,
    Redstone,
}

impl Tab {
    fn category(self) -> Option<CraftingCategory> {
        match self {
            Self::Search => None,
            Self::Equipment => Some(CraftingCategory::Equipment),
            Self::Building => Some(CraftingCategory::Building),
            Self::Misc => Some(CraftingCategory::Misc),
            Self::Redstone => Some(CraftingCategory::Redstone),
        }
    }
}

#[derive(Clone, Debug)]
struct Group {
    recipes: Vec<usize>,
    craftable: Vec<usize>,
}

impl Group {
    fn selected(&self, filtering: bool) -> &[usize] {
        if filtering {
            &self.craftable
        } else {
            &self.recipes
        }
    }

    fn overlay(&self, anchor: usize, filtering: bool) -> Overlay {
        let mut entries: Vec<_> = self
            .craftable
            .iter()
            .map(|&recipe| (recipe, true))
            .collect();
        if !filtering {
            entries.extend(
                self.recipes
                    .iter()
                    .filter(|recipe| self.craftable.binary_search(recipe).is_err())
                    .map(|&recipe| (recipe, false)),
            );
        }
        Overlay { anchor, entries }
    }
}

struct Overlay {
    anchor: usize,
    entries: Vec<(usize, bool)>,
}

impl Overlay {
    fn columns(&self) -> usize {
        if self.entries.len() > 16 { 5 } else { 4 }
    }

    fn panel(&self, w: i32, h: i32) -> Rect {
        overlay_panel(w, h, self.anchor, self.entries.len())
    }

    fn button(&self, w: i32, h: i32, index: usize) -> Rect {
        let panel = self.panel(w, h);
        let columns = self.columns();
        rect(
            panel.x + 4 + (index % columns) as i32 * 25,
            panel.y + 5 + (index / columns) as i32 * 25,
            24,
            24,
        )
    }

    fn hit(&self, w: i32, h: i32, cursor: (i32, i32)) -> Option<usize> {
        (0..self.entries.len()).find(|&i| self.button(w, h, i).contains(cursor.0, cursor.1))
    }
}

// The 26.3 overlay aligns to the selected page button and moves in 25-pixel
// steps when it approaches the edges of the recipe-book panel.
fn overlay_panel(w: i32, h: i32, anchor: usize, count: usize) -> Rect {
    let (bx, by) = book_origin(w, h);
    let columns = if count > 16 { 5 } else { 4 };
    let rows = count.div_ceil(columns);
    let width = count.min(columns) as i32 * 25;
    let height = rows as i32 * 25;
    let mut x = bx + 11 + (anchor % 5) as i32 * 25;
    let mut y = by + 31 + (anchor / 5) as i32 * 25;
    let center_x = bx + 147 / 2;
    let center_y = by + 13 + 166 / 2;
    if x + width > center_x + 50 {
        x -= ((x + width - center_x - 50) / 25) * 25;
    }
    if y + height > center_y + 50 {
        y -= ((y + height - center_y - 50 + 24) / 25) * 25;
    }
    if y < center_y - 100 {
        y += ((center_y - 100 - y) / 25) * 25;
    }
    rect(x, y, width + 8, height + 8)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookAction {
    None,
    Consumed,
    Settings {
        open: bool,
        filtering: bool,
    },
    Place {
        recipe: usize,
        use_max: bool,
        close: bool,
        filtering: bool,
    },
}

pub struct RecipeBookUi {
    recipes: Option<Arc<CraftingRecipes>>,
    initialized: bool,
    open: bool,
    filtering: bool,
    tab: Tab,
    search: String,
    search_focused: bool,
    page: usize,
    cache_revision: Option<u64>,
    visible_tabs: Vec<Tab>,
    groups: Vec<Group>,
    overlay: Option<Overlay>,
    visual_revision: u64,
    display_tick: u64,
}

impl Default for RecipeBookUi {
    fn default() -> Self {
        Self {
            recipes: None,
            initialized: false,
            open: false,
            filtering: false,
            tab: Tab::Search,
            search: String::new(),
            search_focused: false,
            page: 0,
            cache_revision: None,
            visible_tabs: vec![Tab::Search],
            groups: Vec::new(),
            overlay: None,
            visual_revision: 0,
            display_tick: 0,
        }
    }
}

impl RecipeBookUi {
    pub fn set_recipes(&mut self, recipes: Option<Arc<CraftingRecipes>>) {
        self.recipes = recipes;
        self.cache_revision = None;
        self.overlay = None;
    }

    pub fn sync(&mut self, inventory: &PlayerInventory, assets: &MenuAssets) {
        if !self.initialized {
            self.open = inventory.recipe_book().open;
            self.filtering = inventory.recipe_book().filtering;
            self.initialized = true;
        }
        if !inventory.is_open() {
            self.search_focused = false;
            if self.overlay.take().is_some() {
                self.visual_revision = self.visual_revision.wrapping_add(1);
            }
        }
        if !self.open || !inventory.is_open() || self.cache_revision == Some(inventory.revision()) {
            return;
        }
        self.cache_revision = Some(inventory.revision());
        self.groups.clear();
        self.visible_tabs.clear();
        self.visible_tabs.push(Tab::Search);
        let Some(recipes) = &self.recipes else {
            return;
        };
        let width = if matches!(inventory.menu(), Some(InventoryMenu::Player)) {
            2
        } else {
            3
        };
        let available = inventory.recipe_book_available();
        let available = RecipeBookInventory::new(&available);
        let mut groups = Vec::<(CraftingCategory, Group)>::new();
        let mut index = BTreeMap::new();
        let query = self.search.to_lowercase();
        for (recipe_index, recipe) in recipes.iter().enumerate() {
            if !inventory.recipe_book().known.contains(&recipe.id)
                || recipe.book_layout(width, width).is_none()
            {
                continue;
            }
            let tab = match recipe.category {
                CraftingCategory::Equipment => Tab::Equipment,
                CraftingCategory::Building => Tab::Building,
                CraftingCategory::Misc => Tab::Misc,
                CraftingCategory::Redstone => Tab::Redstone,
            };
            if !self.visible_tabs.contains(&tab) {
                self.visible_tabs.push(tab);
            }
            if self
                .tab
                .category()
                .is_some_and(|category| category != recipe.category)
            {
                continue;
            }
            let id = format!("{}:{}", recipe.result.id.namespace, recipe.result.id.path);
            if !query.is_empty() {
                let item_key = format!(
                    "item.{}.{}",
                    recipe.result.id.namespace, recipe.result.id.path
                );
                let block_key = format!(
                    "block.{}.{}",
                    recipe.result.id.namespace, recipe.result.id.path
                );
                let name = assets
                    .language
                    .get(&item_key)
                    .or_else(|| assets.language.get(&block_key));
                if !id.to_lowercase().contains(&query)
                    && !name.is_some_and(|name| name.to_lowercase().contains(&query))
                {
                    continue;
                }
            }
            let can = recipe
                .book_ingredients_counted(width, width, &recipes.tags, &available, 1)
                .is_some();
            let key = (
                recipe.category,
                if recipe.group.is_empty() {
                    format!("\0{}:{}", recipe.id.namespace, recipe.id.path)
                } else {
                    recipe.group.clone()
                },
            );
            let group_index = *index.entry(key).or_insert_with(|| {
                groups.push((
                    recipe.category,
                    Group {
                        recipes: Vec::new(),
                        craftable: Vec::new(),
                    },
                ));
                groups.len() - 1
            });
            groups[group_index].1.recipes.push(recipe_index);
            if can {
                groups[group_index].1.craftable.push(recipe_index);
            }
        }
        self.visible_tabs.sort_by_key(|tab| match tab {
            Tab::Search => 0,
            Tab::Equipment => 1,
            Tab::Building => 2,
            Tab::Misc => 3,
            Tab::Redstone => 4,
        });
        self.groups = groups
            .into_iter()
            .map(|(_, group)| group)
            .filter(|group| !self.filtering || !group.craftable.is_empty())
            .collect();
        let pages = self.groups.len().div_ceil(20).max(1);
        if self.page >= pages {
            self.page = 0;
        }
    }

    fn invalidate(&mut self) {
        self.cache_revision = None;
        self.visual_revision = self.visual_revision.wrapping_add(1);
    }
    pub fn revision(&self) -> u64 {
        self.visual_revision
    }
    pub fn set_tick(&mut self, tick: u64) {
        self.display_tick = tick;
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn hover_state(&self, w: i32, h: i32, cursor: (i32, i32)) -> usize {
        if !self.open {
            return 0;
        }
        if let Some(overlay) = &self.overlay {
            return overlay.hit(w, h, cursor).map_or(0, |index| index + 4);
        }
        let (x, y) = book_origin(w, h);
        if rect(x + 110, y + 12, 26, 16).contains(cursor.0, cursor.1) {
            1
        } else if self.page > 0 && rect(x + 38, y + 137, 12, 17).contains(cursor.0, cursor.1) {
            2
        } else if (self.page + 1) * 20 < self.groups.len()
            && rect(x + 93, y + 137, 12, 17).contains(cursor.0, cursor.1)
        {
            3
        } else {
            0
        }
    }
    pub fn ghost<'a>(
        &'a self,
        inventory: &PlayerInventory,
    ) -> Option<(
        &'a native_resources::recipes::CraftingRecipe,
        &'a CraftingRecipes,
    )> {
        let recipes = self.recipes.as_deref()?;
        Some((recipes.get(inventory.ghost_recipe()?)?, recipes))
    }
    pub fn captures_text(&self) -> bool {
        self.open && self.search_focused
    }
    pub fn redraw_period(&self) -> u64 {
        if self.captures_text() { 10 } else { 30 }
    }
    pub fn backspace(&mut self) {
        if self.search.pop().is_some() {
            self.page = 0;
            self.invalidate();
        }
    }
    pub fn input_text(&mut self, text: &str) {
        let remaining = 50_usize.saturating_sub(self.search.chars().count());
        let addition: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(remaining)
            .collect();
        if !addition.is_empty() {
            self.search.push_str(&addition);
            self.page = 0;
            self.invalidate();
        }
    }
    pub fn unfocus(&mut self) {
        self.search_focused = false;
        self.visual_revision = self.visual_revision.wrapping_add(1);
    }
    pub fn close_narrow(&mut self, width: i32) -> Option<BookAction> {
        if self.open && width < 379 {
            self.open = false;
            self.search_focused = false;
            self.overlay = None;
            self.invalidate();
            Some(BookAction::Settings {
                open: false,
                filtering: self.filtering,
            })
        } else {
            None
        }
    }

    pub fn click(
        &mut self,
        w: i32,
        h: i32,
        cursor: (i32, i32),
        menu: InventoryMenu,
        button: ClickButton,
        use_max: bool,
    ) -> BookAction {
        if matches!(menu, InventoryMenu::Chest { .. }) {
            return BookAction::None;
        }
        if self.open
            && let Some(overlay) = &self.overlay
        {
            if button == ClickButton::Primary
                && let Some(index) = overlay.hit(w, h, cursor)
            {
                let recipe = overlay.entries[index].0;
                let close = w < 379;
                if close {
                    self.open = false;
                    self.overlay = None;
                    self.invalidate();
                }
                return BookAction::Place {
                    recipe,
                    use_max,
                    close,
                    filtering: self.filtering,
                };
            }
            self.overlay = None;
            self.visual_revision = self.visual_revision.wrapping_add(1);
            return BookAction::Consumed;
        }
        let (mx, _) = inventory_origin(w, h, self.open);
        let (bx, by) = book_origin(w, h);
        let button_position = match menu {
            InventoryMenu::Player => (mx + 104, h / 2 - 22),
            InventoryMenu::CraftingTable { .. } => (mx + 5, h / 2 - 49),
            InventoryMenu::Chest { .. } => unreachable!("handled above"),
        };
        if button == ClickButton::Primary
            && !(self.open && w < 379)
            && rect(button_position.0, button_position.1, 20, 18).contains(cursor.0, cursor.1)
        {
            self.open = !self.open;
            self.search_focused = false;
            self.overlay = None;
            self.page = 0;
            self.invalidate();
            return BookAction::Settings {
                open: self.open,
                filtering: self.filtering,
            };
        }
        if !self.open {
            return BookAction::None;
        }
        if button == ClickButton::Primary
            && rect(bx + 8, by + 13, 98, 14).contains(cursor.0, cursor.1)
        {
            self.search_focused = true;
            self.visual_revision = self.visual_revision.wrapping_add(1);
            return BookAction::Consumed;
        }
        self.search_focused = false;
        if button == ClickButton::Primary
            && rect(bx + 110, by + 12, 26, 16).contains(cursor.0, cursor.1)
        {
            self.filtering = !self.filtering;
            self.page = 0;
            self.invalidate();
            return BookAction::Settings {
                open: self.open,
                filtering: self.filtering,
            };
        }
        for (index, tab) in self.visible_tabs.iter().copied().enumerate() {
            if button == ClickButton::Primary
                && rect(bx - 30, by + 3 + index as i32 * 27, 35, 27).contains(cursor.0, cursor.1)
            {
                self.tab = tab;
                self.page = 0;
                self.invalidate();
                return BookAction::Consumed;
            }
        }
        let total_pages = self.groups.len().div_ceil(20).max(1);
        if button == ClickButton::Primary && total_pages > 1 {
            if self.page + 1 < total_pages
                && rect(bx + 93, by + 137, 12, 17).contains(cursor.0, cursor.1)
            {
                self.page += 1;
                self.visual_revision = self.visual_revision.wrapping_add(1);
                return BookAction::Consumed;
            }
            if self.page > 0 && rect(bx + 38, by + 137, 12, 17).contains(cursor.0, cursor.1) {
                self.page -= 1;
                self.visual_revision = self.visual_revision.wrapping_add(1);
                return BookAction::Consumed;
            }
        }
        for index in 0..20 {
            let x = bx + 11 + (index % 5) as i32 * 25;
            let y = by + 31 + (index / 5) as i32 * 25;
            if rect(x, y, 25, 25).contains(cursor.0, cursor.1)
                && let Some(group) = self.groups.get(self.page * 20 + index)
            {
                let choices = group.selected(self.filtering);
                if choices.is_empty() {
                    return BookAction::Consumed;
                }
                if button == ClickButton::Primary {
                    let recipe = choices[(self.display_tick as usize / 30) % choices.len()];
                    let close = w < 379;
                    if close {
                        self.open = false;
                        self.invalidate();
                    }
                    return BookAction::Place {
                        recipe,
                        use_max,
                        close,
                        filtering: self.filtering,
                    };
                }
                if choices.len() > 1 {
                    self.overlay = Some(group.overlay(index, self.filtering));
                    self.visual_revision = self.visual_revision.wrapping_add(1);
                }
                return BookAction::Consumed;
            }
        }
        if self.contains(w, h, cursor) || w < 379 {
            BookAction::Consumed
        } else {
            BookAction::None
        }
    }

    pub fn contains(&self, w: i32, h: i32, cursor: (i32, i32)) -> bool {
        if !self.open {
            return false;
        }
        let (x, y) = book_origin(w, h);
        rect(x, y, 147, 166).contains(cursor.0, cursor.1)
            || rect(x - 30, y + 3, 35, self.visible_tabs.len() as i32 * 27)
                .contains(cursor.0, cursor.1)
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas,
        assets: &MenuAssets,
        models: &mut ItemModelCache,
        cursor: (i32, i32),
        tick: u64,
    ) {
        if !self.open {
            return;
        }
        let (x, y) = book_origin(canvas.width, canvas.height);
        let sprites = &assets.recipe_book;
        canvas.image(&sprites.panel, rect(x, y, 147, 166));
        let filter = rect(x + 110, y + 12, 26, 16);
        let hovered = filter.contains(cursor.0, cursor.1);
        canvas.image(
            match (self.filtering, hovered) {
                (true, true) => &sprites.filter_enabled_highlighted,
                (true, false) => &sprites.filter_enabled,
                (false, true) => &sprites.filter_disabled_highlighted,
                (false, false) => &sprites.filter_disabled,
            },
            filter,
        );
        for (index, tab) in self.visible_tabs.iter().copied().enumerate() {
            let tab_rect = rect(x - 30, y + 3 + index as i32 * 27, 35, 27);
            canvas.image(
                if tab == self.tab {
                    &sprites.tab_selected
                } else {
                    &sprites.tab
                },
                tab_rect,
            );
            if tab == Tab::Search {
                canvas.image(
                    &sprites.compass_icon,
                    rect(tab_rect.x + 9, tab_rect.y + 5, 16, 16),
                );
                continue;
            }
            let icons: &[&str] = match tab {
                Tab::Search => unreachable!(),
                Tab::Equipment => &["minecraft:iron_axe", "minecraft:golden_sword"],
                Tab::Building => &["minecraft:bricks"],
                Tab::Misc => &["minecraft:lava_bucket", "minecraft:apple"],
                Tab::Redstone => &["minecraft:redstone"],
            };
            for (i, id) in icons.iter().enumerate() {
                let sx = tab_rect.x
                    + if icons.len() == 1 {
                        9
                    } else {
                        2 + i as i32 * 14
                    };
                if let Some(item) = icon_stack(id, 1) {
                    stack_icon(canvas, assets, models, sx, tab_rect.y + 5, &item);
                }
            }
        }
        let label = if self.search.is_empty() && !self.search_focused {
            assets.translate("gui.recipebook.search_hint", "Search...")
        } else {
            &self.search
        };
        let mut start = 0;
        while canvas.text_width(Some(assets), &label[start..]) > 79 {
            let Some(next) = label[start..].chars().next() else {
                break;
            };
            start += next.len_utf8();
        }
        let label = &label[start..];
        canvas.text_without_shadow(
            Some(assets),
            x + 25,
            y + 16,
            label,
            if self.search.is_empty() {
                [150, 150, 150]
            } else {
                [255, 255, 255]
            },
        );
        if self.search_focused && (tick / 10).is_multiple_of(2) {
            let caret = x + 25 + canvas.text_width(Some(assets), label);
            canvas.fill(rect(caret, y + 15, 1, 9), [224, 224, 224, 255]);
        }
        let Some(recipes) = &self.recipes else {
            return;
        };
        for (index, group) in self.groups.iter().skip(self.page * 20).take(20).enumerate() {
            let sx = x + 11 + (index % 5) as i32 * 25;
            let sy = y + 31 + (index / 5) as i32 * 25;
            let craftable = !group.craftable.is_empty();
            let choices = group.selected(self.filtering);
            if choices.is_empty() {
                continue;
            }
            let many = choices.len() > 1;
            canvas.image(
                match (craftable, many) {
                    (true, true) => &sprites.slot_many_craftable,
                    (true, false) => &sprites.slot_craftable,
                    (false, true) => &sprites.slot_many_uncraftable,
                    (false, false) => &sprites.slot_uncraftable,
                },
                rect(sx, sy, 25, 25),
            );
            if let Some(recipe) = recipes.at(choices[(tick as usize / 30) % choices.len()]) {
                let id = format!("{}:{}", recipe.result.id.namespace, recipe.result.id.path);
                if let Some(item) = icon_stack(&id, 1) {
                    stack_icon(canvas, assets, models, sx + 4, sy + 4, &item);
                }
            }
        }
        let pages = self.groups.len().div_ceil(20);
        if pages > 1 {
            if self.page > 0 {
                let r = rect(x + 38, y + 137, 12, 17);
                canvas.image(
                    if r.contains(cursor.0, cursor.1) {
                        &sprites.page_backward_highlighted
                    } else {
                        &sprites.page_backward
                    },
                    r,
                );
            }
            if self.page + 1 < pages {
                let r = rect(x + 93, y + 137, 12, 17);
                canvas.image(
                    if r.contains(cursor.0, cursor.1) {
                        &sprites.page_forward_highlighted
                    } else {
                        &sprites.page_forward
                    },
                    r,
                );
            }
            canvas.text_centered(
                Some(assets),
                x + 73,
                y + 141,
                &format!("{}/{}", self.page + 1, pages),
                [255, 255, 255],
            );
        }
        if let Some(overlay) = &self.overlay {
            canvas.gui_sprite(
                &sprites.overlay_panel.image,
                overlay.panel(canvas.width, canvas.height),
                sprites.overlay_panel.scaling,
            );
            let mut item_canvas = Canvas::new(16, 16);
            for (index, &(recipe_index, craftable)) in overlay.entries.iter().enumerate() {
                let button = overlay.button(canvas.width, canvas.height, index);
                let hovered = button.contains(cursor.0, cursor.1);
                let sprite = match (craftable, hovered) {
                    (true, true) => &sprites.overlay_craftable_highlighted,
                    (true, false) => &sprites.overlay_craftable,
                    (false, true) => &sprites.overlay_uncraftable_highlighted,
                    (false, false) => &sprites.overlay_uncraftable,
                };
                canvas.gui_sprite(&sprite.image, button, sprite.scaling);
                let Some(recipe) = recipes.at(recipe_index) else {
                    continue;
                };
                let Some(layout) = recipe.book_layout(3, 3) else {
                    continue;
                };
                for (slot, ingredient) in layout.iter().enumerate() {
                    let Some(ingredient) = ingredient else {
                        continue;
                    };
                    let choices = ingredient.display_items(&recipes.tags);
                    if choices.is_empty() {
                        continue;
                    }
                    let id = choices[(tick as usize / 30) % choices.len()];
                    let x = button.x + 2 + (slot % 3) as i32 * 7;
                    let y = button.y + 2 + (slot / 3) as i32 * 7;
                    if let Some(icon) = models.get(id).and_then(|model| model.gui_icon.clone()) {
                        canvas.image(&icon, rect(x, y, 6, 6));
                        continue;
                    }
                    let Some(item) = icon_stack(id, 1) else {
                        continue;
                    };
                    item_canvas.pixels.fill(0);
                    stack_icon(&mut item_canvas, assets, models, 0, 0, &item);
                    for dy in 0..6 {
                        for dx in 0..6 {
                            let source =
                                item_canvas.pixels[((dy * 16 / 6) * 16 + dx * 16 / 6) as usize];
                            canvas.pixel(
                                x + dx,
                                y + dy,
                                [
                                    (source >> 16) as u8,
                                    (source >> 8) as u8,
                                    source as u8,
                                    (source >> 24) as u8,
                                ],
                            );
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn icon_stack(id: &str, count: u32) -> Option<ItemStack> {
    let components = Tag::Compound(vec![]);
    ordinary_item_default_max(id)
        .and_then(|max| ItemStack::new(id.to_owned(), count, max, components.clone()))
        .or_else(|| ItemStack::unresolved(id.to_owned(), count, components))
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

pub fn inventory_origin(w: i32, h: i32, book_open: bool) -> (i32, i32) {
    let x = if book_open && w >= 379 {
        177 + (w - 176 - 200) / 2
    } else {
        (w - 176) / 2
    };
    (x, (h - 166) / 2)
}

fn book_origin(w: i32, h: i32) -> (i32, i32) {
    ((w - 147) / 2 - if w < 379 { 0 } else { 86 }, (h - 166) / 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positions_follow_vanilla_wide_and_narrow_layout() {
        assert_eq!(inventory_origin(426, 240, false), (125, 37));
        assert_eq!(inventory_origin(426, 240, true), (202, 37));
        assert_eq!(book_origin(426, 240), (53, 37));
        assert_eq!(inventory_origin(320, 240, true), (72, 37));
        assert_eq!(book_origin(320, 240), (86, 37));
    }

    #[test]
    fn grouped_recipes_open_overlay_and_select_craftable_first() {
        let mut ui = RecipeBookUi {
            open: true,
            initialized: true,
            groups: vec![Group {
                recipes: vec![10, 11, 12],
                craftable: vec![11],
            }],
            ..RecipeBookUi::default()
        };
        let menu = InventoryMenu::Player;
        let group_button = (65, 69);
        assert_eq!(
            ui.click(426, 240, group_button, menu, ClickButton::Secondary, false),
            BookAction::Consumed
        );
        let overlay = ui.overlay.as_ref().unwrap();
        assert_eq!(overlay.entries, [(11, true), (10, false), (12, false)]);
        let second = overlay.button(426, 240, 1);
        assert_eq!(ui.hover_state(426, 240, (second.x + 1, second.y + 1)), 5);
        assert_eq!(
            ui.click(
                426,
                240,
                (second.x + 1, second.y + 1),
                menu,
                ClickButton::Primary,
                true
            ),
            BookAction::Place {
                recipe: 10,
                use_max: true,
                close: false,
                filtering: false
            }
        );
        assert!(
            ui.overlay.is_some(),
            "wide overlay stays open after selecting"
        );
        assert_eq!(
            ui.click(426, 240, (400, 220), menu, ClickButton::Primary, false),
            BookAction::Consumed
        );
        assert!(ui.overlay.is_none(), "outside click closes the overlay");
        ui.set_tick(30);
        assert_eq!(
            ui.click(426, 240, group_button, menu, ClickButton::Primary, false),
            BookAction::Place {
                recipe: 11,
                use_max: false,
                close: false,
                filtering: false
            },
            "the unfiltered button cycles through all selected recipes"
        );
        ui.filtering = true;
        assert_eq!(
            ui.click(426, 240, group_button, menu, ClickButton::Secondary, false),
            BookAction::Consumed
        );
        assert!(ui.overlay.is_none(), "one filtered recipe has no overlay");
    }

    #[test]
    fn overlay_selection_closes_the_book_on_narrow_screens() {
        let mut ui = RecipeBookUi {
            open: true,
            initialized: true,
            groups: vec![Group {
                recipes: vec![1, 2],
                craftable: vec![1, 2],
            }],
            ..RecipeBookUi::default()
        };
        let menu = InventoryMenu::Player;
        assert_eq!(
            ui.click(320, 240, (98, 69), menu, ClickButton::Secondary, false),
            BookAction::Consumed
        );
        let button = ui.overlay.as_ref().unwrap().button(320, 240, 0);
        assert_eq!(
            ui.click(
                320,
                240,
                (button.x + 1, button.y + 1),
                menu,
                ClickButton::Primary,
                false
            ),
            BookAction::Place {
                recipe: 1,
                use_max: false,
                close: true,
                filtering: false
            }
        );
        assert!(!ui.open);
        assert!(ui.overlay.is_none());
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn grouped_overlay_layout_matches_vanilla_26_3() {
        use serde_json::{Value, json};
        use std::{ffi::OsString, path::PathBuf, process::Command};

        fn run(command: &mut Command) {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "Java overlay oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("RecipeOverlayOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/fixtures/RecipeOverlayOracle.java"),
        )
        .unwrap();
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
        let mut cases = Vec::new();
        for width in [320, 426] {
            for height in [240, 300] {
                for anchor in [0, 4, 5, 9, 10, 14, 15, 19] {
                    for count in [2, 3, 4, 5, 8, 16, 17, 24, 32] {
                        for filtering in [false, true] {
                            cases.push(json!({"width": width, "height": height, "anchor": anchor, "count": count, "filtering": filtering}));
                        }
                    }
                }
            }
        }
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("RecipeOverlayOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(expected.len(), cases.len());
        for (case, expected) in cases.iter().zip(expected) {
            let width = case["width"].as_i64().unwrap() as i32;
            let height = case["height"].as_i64().unwrap() as i32;
            let anchor = case["anchor"].as_u64().unwrap() as usize;
            let count = case["count"].as_u64().unwrap() as usize;
            let group = Group {
                recipes: (0..count).collect(),
                craftable: (0..count).step_by(2).collect(),
            };
            let overlay = group.overlay(anchor, case["filtering"].as_bool().unwrap());
            let panel = overlay.panel(width, height);
            let buttons: Vec<_> = overlay
                .entries
                .iter()
                .enumerate()
                .map(|(i, &(recipe, craftable))| {
                    let button = overlay.button(width, height, i);
                    json!([button.x, button.y, recipe, craftable])
                })
                .collect();
            assert_eq!(
                json!({"x": panel.x, "y": panel.y, "buttons": buttons}),
                expected,
                "{case}"
            );
        }
        println!(
            "Compared {} grouped recipe overlay layouts with Java 26.3",
            cases.len()
        );
    }

    #[test]
    #[ignore = "performance measurement using installed Minecraft 26.3 assets"]
    fn measure_full_recipe_book_refresh() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = MenuAssets::load(&directory, &[]).unwrap();
        let mut resources = native_resources::packs::ResourceStack::open(&directory).unwrap();
        let recipes = Arc::new(CraftingRecipes::load(&mut resources).unwrap());
        let mut book = native_session::inventory::RecipeBookState::default();
        book.known
            .extend(recipes.iter().map(|recipe| recipe.id.clone()));
        book.open = true;
        let mut inventory = PlayerInventory::default();
        inventory.set_recipe_book(book);
        for (index, id) in [
            "minecraft:stone",
            "minecraft:dirt",
            "minecraft:oak_log",
            "minecraft:oak_planks",
            "minecraft:stick",
            "minecraft:diamond",
            "minecraft:iron_ingot",
            "minecraft:redstone",
            "minecraft:string",
        ]
        .into_iter()
        .cycle()
        .take(36)
        .enumerate()
        {
            inventory.set_slot(
                native_session::inventory::PlayerSlot::Storage(index as u8),
                icon_stack(id, 32),
            );
        }
        inventory.open_crafting_table(glam::IVec3::ZERO);
        let mut ui = RecipeBookUi::default();
        ui.set_recipes(Some(Arc::clone(&recipes)));
        ui.sync(&inventory, &assets);
        let mut durations = Vec::new();
        for index in 0..40 {
            inventory.select((index % 9) as u8);
            let start = std::time::Instant::now();
            ui.sync(&inventory, &assets);
            durations.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        durations.sort_by(f64::total_cmp);
        println!(
            "{} recipes, {} groups; refresh median {:.3} ms, p95 {:.3} ms",
            recipes.len(),
            ui.groups.len(),
            durations[durations.len() / 2],
            durations[durations.len() * 95 / 100]
        );
    }
}
