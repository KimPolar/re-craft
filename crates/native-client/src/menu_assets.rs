//! Vanilla menu textures are read from the user's verified installation at runtime.

use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use native_resources::{
    asset_index::AssetIndex,
    packs::{ResourceProvider, ResourceStack},
};
use serde_json::Value;

pub struct MenuAssets {
    pub minecraft_dir: std::path::PathBuf,
    pub chat: crate::chat_hud::ChatSettings,
    pub item_models: Arc<Mutex<crate::held_items::Loader>>,
    pub trim_registry: Arc<crate::equipment_visual::TrimRegistry>,
    pub main_hand_left: bool,
    pub font: RgbaImage,
    pub nonlatin_font: BitmapFont,
    pub logo: RgbaImage,
    pub edition: RgbaImage,
    pub button: RgbaImage,
    pub button_highlighted: RgbaImage,
    pub button_disabled: RgbaImage,
    pub slider: GuiSprite,
    pub slider_highlighted: GuiSprite,
    pub slider_handle: GuiSprite,
    pub slider_handle_highlighted: GuiSprite,
    pub scroller: GuiSprite,
    pub scroller_background: GuiSprite,
    pub background: RgbaImage,
    pub unknown_pack: RgbaImage,
    pub select_pack: RgbaImage,
    pub unselect_pack: RgbaImage,
    pub hotbar: RgbaImage,
    pub hotbar_selection: RgbaImage,
    pub hotbar_offhand_left: RgbaImage,
    pub hotbar_offhand_right: RgbaImage,
    pub hud: HudSprites,
    pub tooltip_background: GuiSprite,
    pub tooltip_frame: GuiSprite,
    pub inventory: RgbaImage,
    pub crafting_table: RgbaImage,
    pub chest: RgbaImage,
    pub double_chest: RgbaImage,
    pub recipe_book_button: RgbaImage,
    pub recipe_book_button_highlighted: RgbaImage,
    pub recipe_book: RecipeBookAssets,
    pub player_skin: Arc<crate::player_skin::PlayerSkin>,
    pub grass_top: RgbaImage,
    pub grass_side: RgbaImage,
    pub dirt: RgbaImage,
    pub stone: RgbaImage,
    pub stone_bricks: RgbaImage,
    pub cobblestone: RgbaImage,
    pub experience_orb: RgbaImage,
    pub item_glint: RgbaImage,
    pub item_glint_blur: bool,
    pub item_glint_clamp: bool,
    pub wood_textures: Vec<RgbaImage>,
    pub soil_textures: Vec<RgbaImage>,
    pub destroy_stages: Vec<RgbaImage>,
    pub panorama: Vec<RgbaImage>,
    pub panorama_overlay: RgbaImage,
    pub unicode: HashMap<char, UnicodeGlyph>,
    pub language: HashMap<String, String>,
    locale: String,
}

pub struct HudSprites {
    pub effect_background: GuiSprite,
    pub effect_background_ambient: GuiSprite,
    pub effect_icons: Vec<GuiSprite>,
    pub inventory_effect_background: GuiSprite,
    pub inventory_effect_background_ambient: GuiSprite,
    pub experience_background: GuiSprite,
    pub experience_progress: GuiSprite,
    pub armor_empty: GuiSprite,
    pub armor_half: GuiSprite,
    pub armor_full: GuiSprite,
    pub heart_container: GuiSprite,
    pub heart_container_hardcore: GuiSprite,
    pub heart_full: GuiSprite,
    pub heart_half: GuiSprite,
    pub heart_hardcore_full: GuiSprite,
    pub heart_hardcore_half: GuiSprite,
    pub heart_poisoned_full: GuiSprite,
    pub heart_poisoned_half: GuiSprite,
    pub heart_poisoned_hardcore_full: GuiSprite,
    pub heart_poisoned_hardcore_half: GuiSprite,
    pub absorption_full: GuiSprite,
    pub absorption_half: GuiSprite,
    pub absorption_hardcore_full: GuiSprite,
    pub absorption_hardcore_half: GuiSprite,
    pub food_empty: GuiSprite,
    pub food_full: GuiSprite,
    pub food_half: GuiSprite,
    pub food_empty_hunger: GuiSprite,
    pub food_full_hunger: GuiSprite,
    pub food_half_hunger: GuiSprite,
    pub air: GuiSprite,
    pub air_bursting: GuiSprite,
    pub air_empty: GuiSprite,
}

impl HudSprites {
    fn load(stack: &mut ResourceStack) -> Result<Self> {
        let effect_icons = crate::status_effect_ui::EFFECTS
            .iter()
            .map(|effect| {
                load_gui_sprite_path(
                    stack,
                    &format!("assets/minecraft/textures/mob_effect/{}.png", effect.id),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let inventory_effect_background = load_gui_sprite_path(
            stack,
            "assets/minecraft/textures/gui/sprites/container/inventory/effect_background.png",
        )?;
        let inventory_effect_background_ambient = load_gui_sprite_path(
            stack,
            "assets/minecraft/textures/gui/sprites/container/inventory/effect_background_ambient.png",
        )?;
        let mut sprite = |name: &str| {
            load_gui_sprite_path(
                stack,
                &format!("assets/minecraft/textures/gui/sprites/hud/{name}.png"),
            )
        };
        Ok(Self {
            effect_background: sprite("effect_background")?,
            effect_background_ambient: sprite("effect_background_ambient")?,
            effect_icons,
            inventory_effect_background,
            inventory_effect_background_ambient,
            experience_background: sprite("experience_bar_background")?,
            experience_progress: sprite("experience_bar_progress")?,
            armor_empty: sprite("armor_empty")?,
            armor_half: sprite("armor_half")?,
            armor_full: sprite("armor_full")?,
            heart_container: sprite("heart/container")?,
            heart_container_hardcore: sprite("heart/container_hardcore")?,
            heart_full: sprite("heart/full")?,
            heart_half: sprite("heart/half")?,
            heart_hardcore_full: sprite("heart/hardcore_full")?,
            heart_hardcore_half: sprite("heart/hardcore_half")?,
            heart_poisoned_full: sprite("heart/poisoned_full")?,
            heart_poisoned_half: sprite("heart/poisoned_half")?,
            heart_poisoned_hardcore_full: sprite("heart/poisoned_hardcore_full")?,
            heart_poisoned_hardcore_half: sprite("heart/poisoned_hardcore_half")?,
            absorption_full: sprite("heart/absorbing_full")?,
            absorption_half: sprite("heart/absorbing_half")?,
            absorption_hardcore_full: sprite("heart/absorbing_hardcore_full")?,
            absorption_hardcore_half: sprite("heart/absorbing_hardcore_half")?,
            food_empty: sprite("food_empty")?,
            food_full: sprite("food_full")?,
            food_half: sprite("food_half")?,
            food_empty_hunger: sprite("food_empty_hunger")?,
            food_full_hunger: sprite("food_full_hunger")?,
            food_half_hunger: sprite("food_half_hunger")?,
            air: sprite("air")?,
            air_bursting: sprite("air_bursting")?,
            air_empty: sprite("air_empty")?,
        })
    }
}

pub(crate) fn validate_panorama_faces(images: &[RgbaImage]) -> Result<()> {
    ensure!(images.len() == 6, "Minecraft panorama needs six faces");
    let (width, height) = images[0].dimensions();
    ensure!(
        width > 0 && width == height,
        "Minecraft panorama faces must be square"
    );
    ensure!(
        images
            .iter()
            .all(|image| image.dimensions() == (width, height)),
        "Minecraft panorama faces must have the same size"
    );
    Ok(())
}

pub struct RecipeBookAssets {
    pub panel: RgbaImage,
    pub compass_icon: RgbaImage,
    pub tab: RgbaImage,
    pub tab_selected: RgbaImage,
    pub slot_craftable: RgbaImage,
    pub slot_uncraftable: RgbaImage,
    pub slot_many_craftable: RgbaImage,
    pub slot_many_uncraftable: RgbaImage,
    pub overlay_panel: GuiSprite,
    pub overlay_craftable: GuiSprite,
    pub overlay_craftable_highlighted: GuiSprite,
    pub overlay_uncraftable: GuiSprite,
    pub overlay_uncraftable_highlighted: GuiSprite,
    pub filter_enabled: RgbaImage,
    pub filter_enabled_highlighted: RgbaImage,
    pub filter_disabled: RgbaImage,
    pub filter_disabled_highlighted: RgbaImage,
    pub page_forward: RgbaImage,
    pub page_forward_highlighted: RgbaImage,
    pub page_backward: RgbaImage,
    pub page_backward_highlighted: RgbaImage,
}

pub struct GuiSprite {
    pub image: RgbaImage,
    pub scaling: GuiSpriteScaling,
}

fn load_gui_sprite(stack: &mut ResourceStack, name: &str) -> Result<GuiSprite> {
    let path = format!("assets/minecraft/textures/gui/sprites/recipe_book/{name}.png");
    load_gui_sprite_path(stack, &path)
}

fn load_widget_sprite(stack: &mut ResourceStack, name: &str) -> Result<GuiSprite> {
    load_gui_sprite_path(
        stack,
        &format!("assets/minecraft/textures/gui/sprites/widget/{name}.png"),
    )
}

fn load_gui_sprite_path(stack: &mut ResourceStack, path: &str) -> Result<GuiSprite> {
    let asset = stack
        .read_asset_with_metadata(path)?
        .with_context(|| format!("Missing {path}"))?;
    Ok(GuiSprite {
        image: image::load_from_memory(&asset.bytes)
            .with_context(|| format!("Invalid image {path}"))?
            .to_rgba8(),
        scaling: GuiSpriteScaling::from_metadata(asset.metadata.as_deref()),
    })
}

/// Scaling metadata applies to GUI sprites independently of their pixel resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiSpriteScaling {
    Stretch,
    Tile {
        width: i32,
        height: i32,
    },
    NineSlice {
        width: i32,
        height: i32,
        border: [i32; 4], // left, top, right, bottom
        stretch_inner: bool,
    },
}

impl GuiSpriteScaling {
    fn from_metadata(bytes: Option<&[u8]>) -> Self {
        fn parse(bytes: &[u8]) -> Option<GuiSpriteScaling> {
            let metadata: Value = serde_json::from_slice(bytes).ok()?;
            let scaling = metadata.get("gui")?.get("scaling")?;
            let kind = scaling.get("type")?.as_str()?;
            let kind = kind.strip_prefix("minecraft:").unwrap_or(kind);
            if kind == "stretch" {
                return Some(GuiSpriteScaling::Stretch);
            }
            let integer = |value: &Value| i32::try_from(value.as_i64()?).ok();
            let width = integer(scaling.get("width")?)?;
            let height = integer(scaling.get("height")?)?;
            if width <= 0 || height <= 0 {
                return None;
            }
            if kind == "tile" {
                return Some(GuiSpriteScaling::Tile { width, height });
            }
            if kind != "nine_slice" {
                return None;
            }
            let value = scaling.get("border")?;
            let border = if let Some(side) = integer(value) {
                [side; 4]
            } else {
                [
                    integer(value.get("left")?)?,
                    integer(value.get("top")?)?,
                    integer(value.get("right")?)?,
                    integer(value.get("bottom")?)?,
                ]
            };
            if border.iter().any(|side| *side < 0)
                || i64::from(border[0]) + i64::from(border[2]) >= i64::from(width)
                || i64::from(border[1]) + i64::from(border[3]) >= i64::from(height)
            {
                return None;
            }
            let stretch_inner = match scaling.get("stretch_inner") {
                None => false,
                Some(value) => value.as_bool()?,
            };
            Some(GuiSpriteScaling::NineSlice {
                width,
                height,
                border,
                stretch_inner,
            })
        }
        bytes.and_then(parse).unwrap_or(Self::Stretch)
    }
}

pub struct UnicodeGlyph {
    pub rows: [u32; 16],
    pub bits: u32,
}

/// The default non-Latin bitmap provider precedes unifont in vanilla's font stack.
pub struct BitmapFont {
    pub image: RgbaImage,
    pub cells: HashMap<char, BitmapGlyph>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct BitmapGlyph {
    pub x: u32,
    pub y: u32,
    pub advance: i32,
}

fn nonlatin_bitmap_font(definition: &[u8], image: RgbaImage) -> Result<BitmapFont> {
    let definition: Value = serde_json::from_slice(definition)?;
    let provider = definition["providers"]
        .as_array()
        .and_then(|providers| {
            providers.iter().find(|p| {
                p["type"] == "bitmap" && p["file"] == "minecraft:font/nonlatin_european.png"
            })
        })
        .context("Missing non-Latin font provider")?;
    let rows = provider["chars"].as_array().context("Missing font rows")?;
    ensure!(!rows.is_empty(), "Empty font rows");
    let columns = rows[0]
        .as_str()
        .context("Invalid font row")?
        .chars()
        .count();
    ensure!(
        columns > 0 && columns <= 1024 && rows.len() <= 1024,
        "Invalid font grid"
    );
    ensure!(
        provider.get("height").and_then(Value::as_u64).unwrap_or(8) == 8,
        "Unsupported font height"
    );
    ensure!(
        provider["ascent"].as_i64() == Some(7),
        "Unsupported font ascent"
    );
    let width = columns as u32 * 8;
    let height = rows.len() as u32 * 8;
    ensure!(
        image.width() >= width
            && image.height() >= height
            && image.width() % width == 0
            && image.height() % height == 0
            && image.width() / width == image.height() / height,
        "Invalid font texture dimensions"
    );
    let factor = image.height() / height;
    let cell_size = 8 * factor;
    let mut cells = HashMap::new();
    for (y, row) in rows.iter().enumerate() {
        let row = row.as_str().context("Invalid font row")?;
        ensure!(
            row.chars().count() == columns,
            "Inconsistent font row length"
        );
        for (x, ch) in row.chars().enumerate() {
            if ch != '\0' {
                let sx = x as u32 * cell_size;
                let sy = y as u32 * cell_size;
                let actual_width = (0..cell_size)
                    .rev()
                    .find(|&column| {
                        (0..cell_size).any(|row| image.get_pixel(sx + column, sy + row).0[3] != 0)
                    })
                    .map_or(0, |column| column + 1);
                // BitmapProvider rounds the scaled occupied width, then adds one.
                let scale = 8.0_f32 / cell_size as f32;
                let advance = ((f64::from(actual_width as f32 * scale) + 0.5) as i32) + 1;
                cells.insert(
                    ch,
                    BitmapGlyph {
                        x: x as u32 * 8,
                        y: y as u32 * 8,
                        advance,
                    },
                );
            }
        }
    }
    let image = if image.width() == width && image.height() == height {
        image
    } else {
        image::imageops::resize(&image, width, height, image::imageops::FilterType::Nearest)
    };
    Ok(BitmapFont { image, cells })
}

impl MenuAssets {
    pub fn load(minecraft_dir: &Path, packs: &[std::path::PathBuf]) -> Result<Self> {
        let mut stack = ResourceStack::open(minecraft_dir)?;
        for pack in packs {
            stack.push_pack(pack)?;
        }
        // Resolve each overlay sprite together with its pack-specific metadata.
        let overlay_panel = load_gui_sprite(&mut stack, "overlay_recipe")?;
        let overlay_craftable = load_gui_sprite(&mut stack, "crafting_overlay")?;
        let overlay_craftable_highlighted =
            load_gui_sprite(&mut stack, "crafting_overlay_highlighted")?;
        let overlay_uncraftable = load_gui_sprite(&mut stack, "crafting_overlay_disabled")?;
        let overlay_uncraftable_highlighted =
            load_gui_sprite(&mut stack, "crafting_overlay_disabled_highlighted")?;
        let slider = load_widget_sprite(&mut stack, "slider")?;
        let slider_highlighted = load_widget_sprite(&mut stack, "slider_highlighted")?;
        let slider_handle = load_widget_sprite(&mut stack, "slider_handle")?;
        let slider_handle_highlighted =
            load_widget_sprite(&mut stack, "slider_handle_highlighted")?;
        let scroller = load_widget_sprite(&mut stack, "scroller")?;
        let scroller_background = load_widget_sprite(&mut stack, "scroller_background")?;
        let hud = HudSprites::load(&mut stack)?;
        let tooltip_background = load_gui_sprite_path(
            &mut stack,
            "assets/minecraft/textures/gui/sprites/tooltip/background.png",
        )?;
        let tooltip_frame = load_gui_sprite_path(
            &mut stack,
            "assets/minecraft/textures/gui/sprites/tooltip/frame.png",
        )?;
        let glint_path = "assets/minecraft/textures/misc/enchanted_glint_item.png";
        let glint_asset = stack
            .read_asset_with_metadata(glint_path)?
            .context("Missing enchanted item glint texture")?;
        let item_glint = image::load_from_memory(&glint_asset.bytes)
            .context("Invalid enchanted item glint texture")?
            .to_rgba8();
        ensure!(
            item_glint.width() > 0 && item_glint.height() > 0,
            "Empty enchanted item glint texture"
        );
        let glint_metadata: Value = glint_asset
            .metadata
            .as_deref()
            .map(serde_json::from_slice)
            .transpose()
            .context("Invalid enchanted item glint metadata")?
            .unwrap_or(Value::Null);
        let glint_texture_metadata = glint_metadata.get("texture");
        let item_glint_blur = glint_texture_metadata
            .and_then(|value| value.get("blur"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let item_glint_clamp = glint_texture_metadata
            .and_then(|value| value.get("clamp"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut image = |path: &str| -> Result<RgbaImage> {
            let bytes = stack
                .read_asset(path)?
                .with_context(|| format!("Missing {path}"))?;
            Ok(image::load_from_memory(&bytes)
                .with_context(|| format!("Invalid image {path}"))?
                .to_rgba8())
        };
        let font = image("assets/minecraft/textures/font/ascii.png")?;
        let nonlatin_image = image("assets/minecraft/textures/font/nonlatin_european.png")?;
        ensure!(
            font.width() >= 128 && font.height() >= 128,
            "Invalid Minecraft font texture"
        );
        let logo = image("assets/minecraft/textures/gui/title/minecraft.png")?;
        let edition = image("assets/minecraft/textures/gui/title/edition.png")?;
        let button = image("assets/minecraft/textures/gui/sprites/widget/button.png")?;
        let button_highlighted =
            image("assets/minecraft/textures/gui/sprites/widget/button_highlighted.png")?;
        let button_disabled =
            image("assets/minecraft/textures/gui/sprites/widget/button_disabled.png")?;
        let background = image("assets/minecraft/textures/gui/menu_background.png")?;
        let unknown_pack = image("assets/minecraft/textures/misc/unknown_pack.png")?;
        let select_pack =
            image("assets/minecraft/textures/gui/sprites/transferable_list/select.png")?;
        let unselect_pack =
            image("assets/minecraft/textures/gui/sprites/transferable_list/unselect.png")?;
        let hotbar = image("assets/minecraft/textures/gui/sprites/hud/hotbar.png")?;
        let hotbar_selection =
            image("assets/minecraft/textures/gui/sprites/hud/hotbar_selection.png")?;
        let hotbar_offhand_left =
            image("assets/minecraft/textures/gui/sprites/hud/hotbar_offhand_left.png")?;
        let hotbar_offhand_right =
            image("assets/minecraft/textures/gui/sprites/hud/hotbar_offhand_right.png")?;
        let inventory_image = image("assets/minecraft/textures/gui/container/inventory.png")?;
        ensure!(
            inventory_image.width() >= 256 && inventory_image.height() >= 256,
            "Invalid inventory texture"
        );
        let inventory = image::imageops::crop_imm(
            &inventory_image,
            0,
            0,
            inventory_image.width() * 176 / 256,
            inventory_image.height() * 166 / 256,
        )
        .to_image();
        let crafting_image = image("assets/minecraft/textures/gui/container/crafting_table.png")?;
        ensure!(
            crafting_image.width() >= 256 && crafting_image.height() >= 256,
            "Invalid crafting table texture"
        );
        let crafting_table = image::imageops::crop_imm(
            &crafting_image,
            0,
            0,
            crafting_image.width() * 176 / 256,
            crafting_image.height() * 166 / 256,
        )
        .to_image();
        let chest_image = image("assets/minecraft/textures/gui/container/generic_54.png")?;
        ensure!(
            chest_image.width() >= 256 && chest_image.height() >= 256,
            "Invalid chest texture"
        );
        let width = chest_image.width() * 176 / 256;
        let top = chest_image.height() * 71 / 256;
        let bottom = chest_image.height() * 96 / 256;
        let mut chest = RgbaImage::new(width, top + bottom);
        image::imageops::replace(
            &mut chest,
            &image::imageops::crop_imm(&chest_image, 0, 0, width, top).to_image(),
            0,
            0,
        );
        image::imageops::replace(
            &mut chest,
            &image::imageops::crop_imm(
                &chest_image,
                0,
                chest_image.height() * 126 / 256,
                width,
                bottom,
            )
            .to_image(),
            0,
            top as i64,
        );
        let double_top = chest_image.height() * 125 / 256;
        let mut double_chest = RgbaImage::new(width, double_top + bottom);
        image::imageops::replace(
            &mut double_chest,
            &image::imageops::crop_imm(&chest_image, 0, 0, width, double_top).to_image(),
            0,
            0,
        );
        image::imageops::replace(
            &mut double_chest,
            &image::imageops::crop_imm(
                &chest_image,
                0,
                chest_image.height() * 126 / 256,
                width,
                bottom,
            )
            .to_image(),
            0,
            double_top as i64,
        );
        let recipe_book_button =
            image("assets/minecraft/textures/gui/sprites/recipe_book/button.png")?;
        let recipe_book_button_highlighted =
            image("assets/minecraft/textures/gui/sprites/recipe_book/button_highlighted.png")?;
        let book_image = image("assets/minecraft/textures/gui/recipe_book.png")?;
        ensure!(
            book_image.width() >= 256 && book_image.height() >= 256,
            "Invalid recipe book texture"
        );
        let panel = image::imageops::crop_imm(
            &book_image,
            book_image.width() / 256,
            book_image.height() / 256,
            book_image.width() * 147 / 256,
            book_image.height() * 166 / 256,
        )
        .to_image();
        let compass_icon = image("assets/minecraft/textures/item/compass_00.png")?;
        let mut book_sprite = |name: &str| {
            image(&format!(
                "assets/minecraft/textures/gui/sprites/recipe_book/{name}.png"
            ))
        };
        let recipe_book = RecipeBookAssets {
            panel,
            compass_icon,
            tab: book_sprite("tab")?,
            tab_selected: book_sprite("tab_selected")?,
            slot_craftable: book_sprite("slot_craftable")?,
            slot_uncraftable: book_sprite("slot_uncraftable")?,
            slot_many_craftable: book_sprite("slot_many_craftable")?,
            slot_many_uncraftable: book_sprite("slot_many_uncraftable")?,
            overlay_panel,
            overlay_craftable,
            overlay_craftable_highlighted,
            overlay_uncraftable,
            overlay_uncraftable_highlighted,
            filter_enabled: book_sprite("filter_enabled")?,
            filter_enabled_highlighted: book_sprite("filter_enabled_highlighted")?,
            filter_disabled: book_sprite("filter_disabled")?,
            filter_disabled_highlighted: book_sprite("filter_disabled_highlighted")?,
            page_forward: book_sprite("page_forward")?,
            page_forward_highlighted: book_sprite("page_forward_highlighted")?,
            page_backward: book_sprite("page_backward")?,
            page_backward_highlighted: book_sprite("page_backward_highlighted")?,
        };
        let grass_top = image("assets/minecraft/textures/block/grass_block_top.png")?;
        let grass_side = image("assets/minecraft/textures/block/grass_block_side.png")?;
        let dirt = image("assets/minecraft/textures/block/dirt.png")?;
        let stone = image("assets/minecraft/textures/block/stone.png")?;
        let stone_bricks = image("assets/minecraft/textures/block/stone_bricks.png")?;
        let cobblestone = image("assets/minecraft/textures/block/cobblestone.png")?;
        let experience_orb =
            image("assets/minecraft/textures/entity/experience/experience_orb.png")?;
        let wood_textures = native_core::wood::Wood::texture_names()
            .map(|name| image(&format!("assets/minecraft/textures/block/{name}")))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let soil_textures = native_resources::DEMO_SOIL_TEXTURES
            .into_iter()
            .chain(["hanging_roots.png"])
            .map(str::to_owned)
            .chain(
                native_core::ore::Ore::ALL
                    .into_iter()
                    .map(|ore| ore.texture_name()),
            )
            .map(|name| image(&format!("assets/minecraft/textures/block/{name}")))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut destroy_stages = Vec::with_capacity(10);
        for stage in 0..10 {
            let path = format!("assets/minecraft/textures/block/destroy_stage_{stage}.png");
            let texture = image(&path)?;
            ensure!(
                texture.width() > 0 && texture.width() == texture.height(),
                "Invalid breaking texture {path}"
            );
            destroy_stages.push(texture);
        }

        let nonlatin_font = stack
            .read_asset("assets/minecraft/font/include/default.json")?
            .and_then(|definition| nonlatin_bitmap_font(&definition, nonlatin_image).ok())
            .unwrap_or_else(|| BitmapFont {
                image: RgbaImage::new(0, 0),
                cells: HashMap::new(),
            });

        // The client JAR contains one-pixel placeholders for the real panorama. Minecraft's
        // launcher installs these images in the content-addressed asset store instead.
        let index = AssetIndex::load(minecraft_dir)?;
        let mut panorama = Vec::new();
        for face in 0..6 {
            let name = format!("minecraft/textures/gui/title/background/panorama_{face}.png");
            let bytes = match stack.read_pack_asset(&format!("assets/{name}"))? {
                Some(bytes) => bytes,
                None => index.read(minecraft_dir, &name)?,
            };
            panorama.push(
                image::load_from_memory(&bytes)
                    .with_context(|| format!("Invalid Minecraft asset: {name}"))?
                    .to_rgba8(),
            );
        }
        validate_panorama_faces(&panorama)?;
        let overlay_name = "minecraft/textures/gui/title/background/panorama_overlay.png";
        let overlay_bytes = match stack.read_pack_asset(&format!("assets/{overlay_name}"))? {
            Some(bytes) => bytes,
            None => index.read(minecraft_dir, overlay_name)?,
        };
        let panorama_overlay = image::load_from_memory(&overlay_bytes)
            .with_context(|| format!("Invalid Minecraft asset: {overlay_name}"))?
            .to_rgba8();
        ensure!(
            panorama_overlay.width() > 0 && panorama_overlay.height() > 0,
            "Empty Minecraft panorama overlay"
        );
        let unicode = index
            .read(minecraft_dir, "minecraft/font/unifont.zip")
            .and_then(|bytes| parse_unicode_font(&bytes))
            .unwrap_or_default();
        let options = fs::read_to_string(minecraft_dir.join("options.txt")).unwrap_or_default();
        let chat = crate::chat_hud::ChatSettings::from_options(&options);
        let main_hand_left = options
            .lines()
            .find_map(|line| line.strip_prefix("mainHand:"))
            .and_then(|value| serde_json::from_str::<String>(value).ok())
            .is_some_and(|value| value == "left");
        let locale = options
            .lines()
            .find_map(|line| line.strip_prefix("lang:").map(str::to_owned))
            .filter(|value| {
                value
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
            .unwrap_or_else(|| "en_us".to_owned());
        let name = format!("minecraft/lang/{locale}.json");
        let language_bytes = stack
            .read_pack_asset(&format!("assets/{name}"))?
            .or_else(|| index.read(minecraft_dir, &name).ok())
            .or_else(|| {
                stack
                    .read_asset("assets/minecraft/lang/en_us.json")
                    .ok()
                    .flatten()
            });
        let language = language_bytes
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let player_skin = Arc::new(crate::player_skin::PlayerSkin::load(&mut stack, None)?);
        let trim_registry = Arc::new(
            crate::equipment_visual::TrimRegistry::load(&mut stack).unwrap_or_else(|error| {
                eprintln!("cannot load trim registry: {error:#}");
                Default::default()
            }),
        );
        let item_models = Arc::new(Mutex::new(crate::held_items::Loader::from_stack(stack)));
        Ok(Self {
            minecraft_dir: minecraft_dir.to_owned(),
            chat,
            item_models,
            trim_registry,
            main_hand_left,
            font,
            nonlatin_font,
            logo,
            edition,
            button,
            button_highlighted,
            button_disabled,
            slider,
            slider_highlighted,
            slider_handle,
            slider_handle_highlighted,
            scroller,
            scroller_background,
            background,
            unknown_pack,
            select_pack,
            unselect_pack,
            hotbar,
            hotbar_selection,
            hotbar_offhand_left,
            hotbar_offhand_right,
            hud,
            tooltip_background,
            tooltip_frame,
            inventory,
            crafting_table,
            chest,
            double_chest,
            recipe_book_button,
            recipe_book_button_highlighted,
            recipe_book,
            player_skin,
            grass_top,
            grass_side,
            dirt,
            stone,
            stone_bricks,
            cobblestone,
            experience_orb,
            item_glint,
            item_glint_blur,
            item_glint_clamp,
            wood_textures,
            soil_textures,
            destroy_stages,
            panorama,
            panorama_overlay,
            unicode,
            language,
            locale,
        })
    }

    pub fn translate<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        self.language
            .get(key)
            .map(String::as_str)
            .or_else(|| native_translation(&self.locale, key))
            .unwrap_or(fallback)
    }
}

fn native_translation(locale: &str, key: &str) -> Option<&'static str> {
    if locale != "ko_kr" {
        return None;
    }
    match key {
        "minecraft_native.recovery.title" => Some("중단된 저장을 복구하시겠습니까?"),
        "minecraft_native.recovery.message" => Some("중단된 저장을 복구해야 합니다."),
        "minecraft_native.recovery.backup" => Some("원본 백업은 보존됩니다."),
        "minecraft_native.recovery.working" => Some("월드 복구 중..."),
        _ => None,
    }
}

fn parse_unicode_font(bytes: &[u8]) -> Result<HashMap<char, UnicodeGlyph>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut data = String::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        if entry.name().ends_with(".hex") {
            ensure!(
                entry.size() <= 16 * 1024 * 1024,
                "Unicode font is too large"
            );
            entry.take(16 * 1024 * 1024 + 1).read_to_string(&mut data)?;
        }
    }
    let mut glyphs = HashMap::new();
    for line in data.lines() {
        let Some((code, bitmap)) = line.split_once(':') else {
            continue;
        };
        let Some(ch) = u32::from_str_radix(code, 16).ok().and_then(char::from_u32) else {
            continue;
        };
        if ch.is_ascii() || bitmap.len() % 16 != 0 {
            continue;
        }
        let digits = bitmap.len() / 16;
        if !matches!(digits, 2 | 4 | 6 | 8) || !bitmap.is_ascii() {
            continue;
        }
        let mut rows = [0; 16];
        let mut valid = true;
        for (i, row) in rows.iter_mut().enumerate() {
            if let Ok(value) = u32::from_str_radix(&bitmap[i * digits..(i + 1) * digits], 16) {
                *row = value;
            } else {
                valid = false;
                break;
            }
        }
        if valid {
            glyphs.insert(
                ch,
                UnicodeGlyph {
                    rows,
                    bits: (digits * 4) as u32,
                },
            );
        }
    }
    Ok(glyphs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nonlatin_bitmap_grid_and_rejects_inconsistent_rows() {
        let definition = br#"{"providers":[{"type":"bitmap","file":"minecraft:font/nonlatin_european.png","ascent":7,"chars":["\u03b1\u03b2","\u0000\u03b3"]}]}"#;
        let font = nonlatin_bitmap_font(definition, RgbaImage::new(16, 16)).unwrap();
        assert_eq!(
            font.cells[&'α'],
            BitmapGlyph {
                x: 0,
                y: 0,
                advance: 1
            }
        );
        assert_eq!(
            font.cells[&'β'],
            BitmapGlyph {
                x: 8,
                y: 0,
                advance: 1
            }
        );
        assert_eq!(
            font.cells[&'γ'],
            BitmapGlyph {
                x: 8,
                y: 8,
                advance: 1
            }
        );
        assert!(!font.cells.contains_key(&'\0'));
        let invalid = br#"{"providers":[{"type":"bitmap","file":"minecraft:font/nonlatin_european.png","ascent":7,"chars":["ab","c"]}]}"#;
        assert!(nonlatin_bitmap_font(invalid, RgbaImage::new(16, 16)).is_err());
    }

    #[test]
    fn bitmap_advance_uses_all_original_high_resolution_pixels() {
        let definition = br#"{"providers":[{"type":"bitmap","file":"minecraft:font/nonlatin_european.png","ascent":7,"chars":["\u03b1\u03b2"]}]}"#;
        let mut image = RgbaImage::new(32, 16);
        image.put_pixel(2, 0, image::Rgba([255, 255, 255, 1]));
        image.put_pixel(16 + 15, 0, image::Rgba([255, 255, 255, 255]));
        let font = nonlatin_bitmap_font(definition, image).unwrap();
        assert_eq!(font.cells[&'α'].advance, 3);
        assert_eq!(font.cells[&'β'].advance, 9);
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn korean_recovery_strings_fit_and_resource_pack_translation_takes_priority() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut assets = MenuAssets::load(&directory, &[]).unwrap();
        assets.locale = "ko_kr".into();
        let canvas = crate::menu_draw::Canvas::new(320, 240);
        for key in [
            "minecraft_native.recovery.title",
            "minecraft_native.recovery.message",
            "minecraft_native.recovery.backup",
            "minecraft_native.recovery.working",
        ] {
            assets.language.remove(key);
            let text = assets.translate(key, "fallback");
            assert_ne!(text, "fallback");
            assert!(
                canvas.text_width(Some(&assets), text) <= 320,
                "{key}: {text}"
            );
            for c in text
                .chars()
                .filter(|c| ('\u{ac00}'..='\u{d7a3}').contains(c))
            {
                assert!(assets.unicode.contains_key(&c), "missing glyph {c}");
            }
        }
        assets.language.insert(
            "minecraft_native.recovery.title".into(),
            "pack override".into(),
        );
        assert_eq!(
            assets.translate("minecraft_native.recovery.title", "fallback"),
            "pack override"
        );
        assets.locale = "en_us".into();
        assert_eq!(
            assets.translate("minecraft_native.recovery.message", "fallback"),
            "fallback"
        );
    }

    #[test]
    fn panorama_faces_must_be_six_equally_sized_squares() {
        let square = RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]));
        let mut faces = vec![square; 6];
        assert!(validate_panorama_faces(&faces).is_ok());
        assert!(validate_panorama_faces(&faces[..5]).is_err());
        faces[4] = RgbaImage::new(8, 7);
        assert!(validate_panorama_faces(&faces).is_err());
        faces[4] = RgbaImage::new(4, 4);
        assert!(validate_panorama_faces(&faces).is_err());
        assert!(validate_panorama_faces(&vec![RgbaImage::new(0, 0); 6]).is_err());
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn panorama_faces_follow_the_resource_pack_and_validate_dimensions() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let background = temp
            .path()
            .join("assets/minecraft/textures/gui/title/background");
        std::fs::create_dir_all(&background).unwrap();
        for face in 0..6 {
            RgbaImage::from_pixel(8, 8, image::Rgba([face * 30, 90, 140, 255]))
                .save(background.join(format!("panorama_{face}.png")))
                .unwrap();
        }
        let assets = MenuAssets::load(&directory, &[temp.path().to_path_buf()]).unwrap();
        for face in 0..6 {
            assert_eq!(
                assets.panorama[face].get_pixel(0, 0).0,
                [face as u8 * 30, 90, 140, 255]
            );
        }
        std::fs::remove_file(background.join("panorama_3.png")).unwrap();
        let error = match MenuAssets::load(&directory, &[temp.path().to_path_buf()]) {
            Ok(_) => panic!("mismatched panorama faces were accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("same size"), "{error:#}");
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn panorama_overlay_follows_the_selected_resource_pack() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let background = temp
            .path()
            .join("assets/minecraft/textures/gui/title/background");
        std::fs::create_dir_all(&background).unwrap();
        RgbaImage::from_pixel(2, 2, image::Rgba([23, 67, 109, 127]))
            .save(background.join("panorama_overlay.png"))
            .unwrap();
        let assets = MenuAssets::load(&directory, &[temp.path().to_path_buf()]).unwrap();
        assert_eq!(assets.panorama_overlay.dimensions(), (2, 2));
        assert_eq!(
            assets.panorama_overlay.get_pixel(0, 0).0,
            [23, 67, 109, 127]
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn enchanted_glint_uses_resource_pack_texture_and_filter_metadata() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let vanilla = MenuAssets::load(&directory, &[]).unwrap();
        assert!(vanilla.item_glint_blur);
        assert!(!vanilla.item_glint_clamp);
        let temp = tempfile::tempdir().unwrap();
        let textures = temp.path().join("assets/minecraft/textures/misc");
        std::fs::create_dir_all(&textures).unwrap();
        let image = textures.join("enchanted_glint_item.png");
        RgbaImage::from_pixel(4, 4, image::Rgba([23, 67, 109, 255]))
            .save(&image)
            .unwrap();
        let packed = MenuAssets::load(&directory, &[temp.path().to_path_buf()]).unwrap();
        assert_eq!(packed.item_glint.get_pixel(0, 0).0, [23, 67, 109, 255]);
        assert!(!packed.item_glint_blur && !packed.item_glint_clamp);
        std::fs::write(
            textures.join("enchanted_glint_item.png.mcmeta"),
            br#"{"texture":{"blur":true,"clamp":true}}"#,
        )
        .unwrap();
        let packed = MenuAssets::load(&directory, &[temp.path().to_path_buf()]).unwrap();
        assert!(packed.item_glint_blur && packed.item_glint_clamp);
    }

    #[test]
    fn gui_sprite_metadata_supports_tiled_and_nine_slice_pack_overrides() {
        let vanilla =
            br#"{"gui":{"scaling":{"type":"nine_slice","width":32,"height":32,"border":4}}}"#;
        assert_eq!(
            GuiSpriteScaling::from_metadata(Some(vanilla)),
            GuiSpriteScaling::NineSlice {
                width: 32,
                height: 32,
                border: [4; 4],
                stretch_inner: false,
            }
        );
        let asymmetric = br#"{"gui":{"scaling":{"type":"nine_slice","width":8,"height":9,"border":{"left":1,"top":2,"right":3,"bottom":4},"stretch_inner":true}}}"#;
        assert_eq!(
            GuiSpriteScaling::from_metadata(Some(asymmetric)),
            GuiSpriteScaling::NineSlice {
                width: 8,
                height: 9,
                border: [1, 2, 3, 4],
                stretch_inner: true,
            }
        );
        let tile = br#"{"gui":{"scaling":{"type":"tile","width":8,"height":12}}}"#;
        assert_eq!(
            GuiSpriteScaling::from_metadata(Some(tile)),
            GuiSpriteScaling::Tile {
                width: 8,
                height: 12
            }
        );
        let invalid =
            br#"{"gui":{"scaling":{"type":"nine_slice","width":8,"height":8,"border":4}}}"#;
        assert_eq!(
            GuiSpriteScaling::from_metadata(Some(invalid)),
            GuiSpriteScaling::Stretch
        );
        assert_eq!(
            GuiSpriteScaling::from_metadata(None),
            GuiSpriteScaling::Stretch
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn recipe_overlay_uses_metadata_from_the_selected_resource_pack() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let sprites = temp
            .path()
            .join("assets/minecraft/textures/gui/sprites/recipe_book");
        std::fs::create_dir_all(&sprites).unwrap();
        let image = sprites.join("overlay_recipe.png");
        RgbaImage::from_pixel(8, 8, image::Rgba([17, 85, 170, 255]))
            .save(&image)
            .unwrap();
        let metadata = sprites.join("overlay_recipe.png.mcmeta");
        std::fs::write(
            &metadata,
            br#"{"gui":{"scaling":{"type":"nine_slice","width":8,"height":8,"border":1}}}"#,
        )
        .unwrap();
        RgbaImage::from_pixel(8, 8, image::Rgba([100, 120, 140, 255]))
            .save(sprites.join("crafting_overlay.png"))
            .unwrap();
        std::fs::write(
            sprites.join("crafting_overlay.png.mcmeta"),
            br#"{"gui":{"scaling":{"type":"tile","width":8,"height":8}}}"#,
        )
        .unwrap();
        let assets = MenuAssets::load(&directory, &[temp.path().to_path_buf()]).unwrap();
        assert_eq!(
            assets.recipe_book.overlay_panel.image.get_pixel(0, 0).0,
            [17, 85, 170, 255]
        );
        assert_eq!(
            assets.recipe_book.overlay_panel.scaling,
            GuiSpriteScaling::NineSlice {
                width: 8,
                height: 8,
                border: [1; 4],
                stretch_inner: false,
            }
        );
        assert_eq!(
            assets.recipe_book.overlay_craftable.scaling,
            GuiSpriteScaling::Tile {
                width: 8,
                height: 8
            }
        );
        std::fs::remove_file(metadata).unwrap();
        let assets = MenuAssets::load(&directory, &[temp.path().to_path_buf()]).unwrap();
        assert_eq!(
            assets.recipe_book.overlay_panel.scaling,
            GuiSpriteScaling::Stretch,
            "a replacement image must not inherit metadata from the vanilla image"
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn world_entity_textures_follow_the_selected_resource_pack() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let textures = temp.path().join("assets/minecraft/textures/block");
        std::fs::create_dir_all(&textures).unwrap();
        RgbaImage::from_pixel(16, 16, image::Rgba([20, 80, 120, 255]))
            .save(textures.join("oak_log.png"))
            .unwrap();
        RgbaImage::from_pixel(16, 16, image::Rgba([130, 70, 30, 255]))
            .save(textures.join("oak_log_top.png"))
            .unwrap();
        RgbaImage::from_pixel(16, 16, image::Rgba([44, 66, 88, 255]))
            .save(textures.join("coarse_dirt.png"))
            .unwrap();
        RgbaImage::from_pixel(16, 16, image::Rgba([71, 92, 113, 255]))
            .save(textures.join("hanging_roots.png"))
            .unwrap();
        RgbaImage::from_pixel(16, 16, image::Rgba([9, 33, 77, 255]))
            .save(textures.join("diamond_ore.png"))
            .unwrap();
        let orb_textures = temp
            .path()
            .join("assets/minecraft/textures/entity/experience");
        std::fs::create_dir_all(&orb_textures).unwrap();
        RgbaImage::from_pixel(64, 64, image::Rgba([9, 123, 45, 255]))
            .save(orb_textures.join("experience_orb.png"))
            .unwrap();
        let assets = MenuAssets::load(&directory, &[temp.path().to_path_buf()]).unwrap();
        let oak = native_core::wood::Wood::from_id("minecraft:oak_log").unwrap();
        let index = oak.texture_index() as usize;
        assert_eq!(
            assets.wood_textures[index].get_pixel(0, 0).0,
            [20, 80, 120, 255]
        );
        assert_eq!(
            assets.wood_textures[index + 1].get_pixel(0, 0).0,
            [130, 70, 30, 255]
        );
        assert_eq!(assets.soil_textures[0].get_pixel(0, 0).0, [44, 66, 88, 255]);
        assert_eq!(
            assets.soil_textures[native_resources::DEMO_SOIL_TEXTURES.len()]
                .get_pixel(0, 0)
                .0,
            [71, 92, 113, 255]
        );
        assert_eq!(assets.experience_orb.get_pixel(0, 0).0, [9, 123, 45, 255]);
        let diamond = native_core::ore::Ore::Diamond;
        assert_eq!(
            assets.soil_textures
                [native_resources::DEMO_SOIL_TEXTURES.len() + 1 + diamond.index() as usize]
                .get_pixel(0, 0)
                .0,
            [9, 33, 77, 255]
        );
    }

    #[test]
    fn reads_unicode_glyphs_from_the_launcher_font_archive() {
        use std::io::Write;
        let output = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(output);
        zip.start_file("font.hex", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(
            b"AC00:0000000000043F8400840084008400870104010402040C043004000400040004\ninvalid\n",
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let font = parse_unicode_font(&bytes).unwrap();
        assert_eq!(font[&'\u{ac00}'].bits, 16);
        assert_eq!(font[&'\u{ac00}'].rows[2], 4);
        assert_eq!(font[&'\u{ac00}'].rows[3], 0x3f84);
    }
}
