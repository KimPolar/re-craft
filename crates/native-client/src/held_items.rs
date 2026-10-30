//! Load ordinary cuboid and generated item models away from the render thread.
//!
//! Item properties and special renderers still need
//! their own resolvers. Unsupported definitions remain absent from this cache.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Cursor,
    sync::{Arc, Mutex, Weak, mpsc},
    thread,
};

use anyhow::{Context, Result, bail, ensure};
use glam::{Quat, Vec3};
use image::{ImageFormat, ImageReader, RgbaImage};
use native_persistence::nbt::Tag;
use native_resources::{
    biomes::ColorMap,
    geometry::{self, Vertex},
    models,
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use native_session::inventory::ItemStack;
use serde_json::Value;

use crate::generated_item;

#[derive(Clone, Copy, Debug)]
pub struct DisplayTransform {
    pub translation: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
}

impl DisplayTransform {
    fn parse(value: Option<&Value>) -> Result<Self> {
        let Some(value) = value else {
            return Ok(Self {
                translation: Vec3::ZERO,
                rotation: Vec3::ZERO,
                scale: Vec3::ONE,
            });
        };
        let object = value
            .as_object()
            .context("display transform must be an object")?;
        let vector = |name: &str, default: Vec3| -> Result<Vec3> {
            let Some(value) = object.get(name) else {
                return Ok(default);
            };
            let values = value
                .as_array()
                .context("display vector must be an array")?;
            ensure!(
                values.len() == 3,
                "display vector must have three coordinates"
            );
            let mut result = [0.0; 3];
            for (out, value) in result.iter_mut().zip(values) {
                *out = value
                    .as_f64()
                    .context("display coordinate must be a number")? as f32;
                ensure!(out.is_finite(), "display coordinate must be finite");
            }
            Ok(Vec3::from_array(result))
        };
        Ok(Self {
            translation: (vector("translation", Vec3::ZERO)? / 16.0)
                .clamp(Vec3::splat(-5.0), Vec3::splat(5.0)),
            rotation: vector("rotation", Vec3::ZERO)?,
            scale: vector("scale", Vec3::ONE)?.clamp(Vec3::splat(-4.0), Vec3::splat(4.0)),
        })
    }

    pub fn rotation(self, left: bool) -> Quat {
        let radians = self.rotation * (std::f32::consts::PI / 180.0);
        let sign = if left { -1.0 } else { 1.0 };
        Quat::from_rotation_x(radians.x)
            * Quat::from_rotation_y(radians.y * sign)
            * Quat::from_rotation_z(radians.z * sign)
    }
}

pub struct TexturedQuad {
    pub vertices: [Vertex; 4],
    pub normal: [f32; 3],
    pub texture: Arc<RgbaImage>,
    pub tint: [f32; 3],
    pub layer: u8,
}

pub struct SpriteLayer {
    pub texture: Arc<RgbaImage>,
    pub tint: [f32; 3],
    pub glint_uv: Option<[f32; 4]>,
}

pub struct HeldModel {
    pub quads: Vec<TexturedQuad>,
    pub numeric_item_id: Option<i32>,
    pub glint: bool,
    /// Atlas coordinates per distinct texture for world and hand rendering.
    pub glint_materials: Vec<(Arc<RgbaImage>, Option<[f32; 4]>)>,
    pub right: DisplayTransform,
    pub left: DisplayTransform,
    pub first_right: DisplayTransform,
    pub first_left: DisplayTransform,
    pub ground: DisplayTransform,
    pub gui: DisplayTransform,
    pub gui_side_light: bool,
    pub gui_icon: Option<Arc<RgbaImage>>,
    /// Atlas bounds for an untransformed single-layer GUI sprite, when known.
    pub gui_glint_uv: Option<[f32; 4]>,
    /// Atlas coordinates at each visible icon pixel for transformed or layered models.
    pub gui_glint_pixels: Option<Box<[[u16; 3]; 256]>>,
    /// Per-face bounds only for glint models that require GUI rasterization.
    pub gui_glint_quads: Option<Vec<Option<[f32; 4]>>>,
    pub sprite_layers: Vec<SpriteLayer>,
}

pub(crate) struct Loader {
    stack: ResourceStack,
    grass: Option<ColorMap>,
    textures: HashMap<String, Weak<RgbaImage>>,
    item_atlas: Option<Option<native_resources::vanilla_stitcher::StitchLayout>>,
    block_atlas: Option<Option<native_resources::vanilla_stitcher::StitchLayout>>,
    item_registry: Option<Option<native_resources::item_registry::ItemRegistry>>,
}

/// Item properties that can change a supported item model or tint.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ModelKey {
    id: String,
    trim_material: Option<String>,
    dye: Option<u32>,
    glint: bool,
}

impl ModelKey {
    fn plain(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            trim_material: None,
            dye: None,
            glint: false,
        }
    }

    fn from_item(item: &ItemStack) -> Self {
        let trim_material = item
            .components()
            .get("minecraft:trim")
            .and_then(|trim| trim.get("material"))
            .and_then(|tag| match tag {
                Tag::String(value) => String::from_utf16(&value.0).ok(),
                _ => None,
            })
            .and_then(|id| ResourceId::parse(&id).ok())
            .map(|id| format!("{}:{}", id.namespace, id.path));
        let dye = item
            .components()
            .get("minecraft:dyed_color")
            .and_then(Tag::as_java_int)
            .map(|value| value as u32 & 0xffffff);
        Self {
            id: item.id().to_owned(),
            trim_material,
            dye,
            glint: item.has_glint(),
        }
    }
}

fn selected_model<'a>(mut entry: &'a Value, key: &ModelKey) -> Result<&'a Value> {
    for _ in 0..32 {
        match entry.get("type").and_then(Value::as_str) {
            Some("minecraft:model" | "model") => return Ok(entry),
            Some("minecraft:select" | "select") => {
                ensure!(
                    matches!(
                        entry.get("property").and_then(Value::as_str),
                        Some("minecraft:trim_material" | "trim_material")
                    ),
                    "unsupported item select property"
                );
                let selected = key.trim_material.as_deref().and_then(|material| {
                    entry
                        .get("cases")?
                        .as_array()?
                        .iter()
                        .find(|case| {
                            let matches = |value: &Value| {
                                value
                                    .as_str()
                                    .and_then(|id| ResourceId::parse(id).ok())
                                    .is_some_and(|id| {
                                        format!("{}:{}", id.namespace, id.path) == material
                                    })
                            };
                            case.get("when").is_some_and(|when| match when {
                                Value::Array(values) => values.iter().any(matches),
                                value => matches(value),
                            })
                        })
                        .and_then(|case| case.get("model"))
                });
                entry = selected
                    .or_else(|| entry.get("fallback"))
                    .context("item select has no matching model")?;
            }
            _ => bail!("unsupported item model type"),
        }
    }
    bail!("item model nesting is too deep")
}

impl Loader {
    pub(crate) fn load_armor(
        &mut self,
        key: &crate::equipment_visual::ArmorKey,
    ) -> Result<crate::equipment_visual::ArmorAppearance> {
        crate::equipment_visual::load(&mut self.stack, key)
    }

    pub(crate) fn from_stack(mut stack: ResourceStack) -> Self {
        let grass = stack
            .read_asset("assets/minecraft/textures/colormap/grass.png")
            .ok()
            .flatten()
            .and_then(|bytes| decode_png(bytes, 256).ok())
            .and_then(|image| ColorMap::new(image).ok());
        Self {
            stack,
            grass,
            textures: HashMap::new(),
            item_atlas: None,
            block_atlas: None,
            item_registry: None,
        }
    }

    fn glint_uv(&mut self, sprite: &ResourceId) -> Option<[f32; 4]> {
        fn bounds(
            layout: &native_resources::vanilla_stitcher::StitchLayout,
            sprite: &ResourceId,
        ) -> Option<[f32; 4]> {
            let position = layout.placements.get(sprite)?;
            let x = position.x + position.padding;
            let y = position.y + position.padding;
            Some([
                x as f32 / layout.width as f32,
                y as f32 / layout.height as f32,
                (x + position.width) as f32 / layout.width as f32,
                (y + position.height) as f32 / layout.height as f32,
            ])
        }
        // Java's MaterialBaker resolves the item atlas before the block atlas.
        let item = self.item_atlas.get_or_insert_with(|| {
            native_resources::vanilla_atlas::item_layout(&mut self.stack).ok()
        });
        let item = item.as_ref()?;
        if let Some(uv) = bounds(item, sprite) {
            return Some(uv);
        }
        let block = self.block_atlas.get_or_insert_with(|| {
            native_resources::vanilla_atlas::block_layout(&mut self.stack, 4).ok()
        });
        bounds(block.as_ref()?, sprite)
    }

    fn texture(&mut self, id: &ResourceId) -> Result<Arc<RgbaImage>> {
        let path = id.asset_path("textures", "png");
        if let Some(image) = self.textures.get(&path).and_then(Weak::upgrade) {
            return Ok(image);
        }
        let image = if let Some(asset) = self.stack.read_asset_with_metadata(&path)? {
            let image = decode_png(asset.bytes, 4096)?;
            // Held item animation is not yet advanced after its initial frame.
            native_resources::sprites::first_frame(&image, asset.metadata.as_deref())?
        } else {
            self.paletted_item_texture(id)?
                .with_context(|| format!("missing {path}"))?
        };
        let image = Arc::new(image);
        self.textures.insert(path, Arc::downgrade(&image));
        if self.textures.len() > 256 {
            self.textures.retain(|_, image| image.strong_count() != 0);
        }
        Ok(image)
    }

    /// Resolve generated item sprites such as armor trims through the active
    /// atlas sources and palette textures instead of assuming a PNG exists.
    fn paletted_item_texture(&mut self, id: &ResourceId) -> Result<Option<RgbaImage>> {
        let atlases = self
            .stack
            .read_asset_stack("assets/minecraft/atlases/items.json")?;
        for atlas in atlases.iter().rev() {
            let atlas: Value = serde_json::from_slice(atlas).context("invalid item atlas JSON")?;
            let Some(sources) = atlas.get("sources").and_then(Value::as_array) else {
                continue;
            };
            for source in sources.iter().rev() {
                if !matches!(
                    source.get("type").and_then(Value::as_str),
                    Some("minecraft:paletted_permutations" | "paletted_permutations")
                ) {
                    continue;
                }
                let Some(textures) = source.get("textures").and_then(Value::as_array) else {
                    continue;
                };
                let Some(permutations) = source.get("permutations").and_then(Value::as_object)
                else {
                    continue;
                };
                for texture in textures {
                    let Some(texture) = texture.as_str() else {
                        continue;
                    };
                    let texture = ResourceId::parse(texture)?;
                    for (name, palette) in permutations {
                        if texture.namespace != id.namespace
                            || format!("{}_{}", texture.path, name) != id.path
                        {
                            continue;
                        }
                        let palette_key = ResourceId::parse(
                            source
                                .get("palette_key")
                                .and_then(Value::as_str)
                                .context("missing palette key")?,
                        )?;
                        let palette =
                            ResourceId::parse(palette.as_str().context("invalid palette id")?)?;
                        let base_path = texture.asset_path("textures", "png");
                        let base = self
                            .stack
                            .read_asset_with_metadata(&base_path)?
                            .context("missing uncolored item sprite")?;
                        let image = decode_png(base.bytes, 4096)?;
                        let image = native_resources::sprites::first_frame(
                            &image,
                            base.metadata.as_deref(),
                        )?;
                        let read_palette =
                            |stack: &mut ResourceStack, id: &ResourceId| -> Result<RgbaImage> {
                                let path = id.asset_path("textures/palettes", "png");
                                decode_png(
                                    stack
                                        .read_asset(&path)?
                                        .with_context(|| format!("missing palette {path}"))?,
                                    4096,
                                )
                            };
                        let source_palette = read_palette(&mut self.stack, &palette_key)?;
                        let target_palette = read_palette(&mut self.stack, &palette)?;
                        return Ok(Some(apply_palette(
                            image,
                            &source_palette,
                            &target_palette,
                        )?));
                    }
                }
            }
        }
        Ok(None)
    }

    #[cfg(test)]
    fn load(&mut self, item: &str) -> Result<HeldModel> {
        self.load_visual(&ModelKey::plain(item))
    }

    fn load_visual(&mut self, key: &ModelKey) -> Result<HeldModel> {
        if self.item_registry.is_none() {
            self.item_registry = Some(self.stack.vanilla_item_registry().ok());
        }
        let id = ResourceId::parse(&key.id)?;
        let numeric_item_id = self
            .item_registry
            .as_ref()
            .and_then(|registry| registry.as_ref())
            .and_then(|registry| registry.id(&format!("{}:{}", id.namespace, id.path)));
        let path = id.asset_path("items", "json");
        let bytes = self
            .stack
            .read_asset(&path)?
            .with_context(|| format!("missing {path}"))?;
        let definition: Value = serde_json::from_slice(&bytes)?;
        let entry = selected_model(definition.get("model").context("missing item model")?, key)?;
        let name = entry
            .get("model")
            .and_then(Value::as_str)
            .context("missing model name")?;
        let model = models::resolve_model(&mut self.stack, name)?;
        let display = model.fields.get("display");
        let right = DisplayTransform::parse(display.and_then(|v| v.get("thirdperson_righthand")))?;
        let left = DisplayTransform::parse(display.and_then(|v| v.get("thirdperson_lefthand")))?;
        let first_right =
            DisplayTransform::parse(display.and_then(|v| v.get("firstperson_righthand")))?;
        let first_left =
            DisplayTransform::parse(display.and_then(|v| v.get("firstperson_lefthand")))?;
        let ground = DisplayTransform::parse(display.and_then(|v| v.get("ground")))?;
        let gui = DisplayTransform::parse(display.and_then(|v| v.get("gui")))?;
        let gui_side_light = match model.fields.get("gui_light") {
            None => true,
            Some(Value::String(value)) if value == "side" => true,
            Some(Value::String(value)) if value == "front" => false,
            Some(_) => bail!("unsupported GUI light mode"),
        };
        let tints = entry.get("tints").and_then(Value::as_array);
        let color_for = |index: usize, loader: &Self| -> Result<[f32; 3]> {
            let color = match tints.and_then(|a| a.get(index)) {
                Some(tint) => loader.tint(tint, key)?,
                None => 0xffffff,
            };
            Ok([
                ((color >> 16) & 255) as f32 / 255.0,
                ((color >> 8) & 255) as f32 / 255.0,
                (color & 255) as f32 / 255.0,
            ])
        };
        let mut sprite_layers = Vec::new();
        let mut quads = Vec::new();
        let mut glint_materials: Vec<(Arc<RgbaImage>, Option<[f32; 4]>)> = Vec::new();
        let flat_gui = model.generated
            && !gui_side_light
            && gui.rotation == Vec3::ZERO
            && gui.translation == Vec3::ZERO
            && gui.scale == Vec3::ONE;
        let mut gui_glint_quads = (key.glint && !flat_gui).then(Vec::new);
        if model.generated {
            let textures = model
                .fields
                .get("textures")
                .and_then(Value::as_object)
                .context("generated item has no textures")?;
            for layer in 0..5_u8 {
                let name = format!("layer{layer}");
                if !textures.contains_key(&name) {
                    break;
                }
                let sprite = model.texture(&name)?.sprite;
                let texture = self.texture(&sprite)?;
                let tint = color_for(usize::from(layer), self)?;
                let glint_uv = key.glint.then(|| self.glint_uv(&sprite)).flatten();
                if key.glint
                    && !glint_materials
                        .iter()
                        .any(|(image, _)| Arc::ptr_eq(image, &texture))
                {
                    glint_materials.push((Arc::clone(&texture), glint_uv));
                }
                let generated = generated_item::bake(Arc::clone(&texture), tint, layer)?;
                if let Some(bounds) = &mut gui_glint_quads {
                    bounds.extend(std::iter::repeat_n(glint_uv, generated.len()));
                }
                quads.extend(generated);
                sprite_layers.push(SpriteLayer {
                    texture,
                    tint,
                    glint_uv,
                });
            }
        } else {
            let baked = geometry::bake_untransformed(&model)?;
            for quad in baked.quads {
                let tint = match usize::try_from(quad.tint_index) {
                    Ok(index) => color_for(index, self)?,
                    Err(_) => [1.0; 3],
                };
                let texture = self.texture(&quad.texture.sprite)?;
                let glint_uv = key
                    .glint
                    .then(|| self.glint_uv(&quad.texture.sprite))
                    .flatten();
                if let Some(bounds) = &mut gui_glint_quads {
                    bounds.push(glint_uv);
                }
                if key.glint
                    && !glint_materials
                        .iter()
                        .any(|(image, _)| Arc::ptr_eq(image, &texture))
                {
                    glint_materials.push((Arc::clone(&texture), glint_uv));
                }
                quads.push(TexturedQuad {
                    vertices: quad.vertices,
                    normal: quad.direction.normal(),
                    texture,
                    tint,
                    layer: 0,
                });
            }
        }
        ensure!(!quads.is_empty(), "item uses an unsupported or empty model");
        let gui_glint_uv = if sprite_layers.len() == 1
            && !gui_side_light
            && gui.rotation == Vec3::ZERO
            && gui.translation == Vec3::ZERO
            && gui.scale == Vec3::ONE
        {
            sprite_layers[0].glint_uv
        } else {
            None
        };
        Ok(HeldModel {
            quads,
            numeric_item_id,
            glint: key.glint,
            glint_materials,
            right,
            left,
            first_right,
            first_left,
            ground,
            gui,
            gui_side_light,
            gui_icon: None,
            gui_glint_uv,
            gui_glint_pixels: None,
            gui_glint_quads,
            sprite_layers,
        })
    }

    fn tint(&self, tint: &Value, key: &ModelKey) -> Result<u32> {
        match tint.get("type").and_then(Value::as_str) {
            Some("minecraft:grass" | "grass") => {
                let temperature = tint
                    .get("temperature")
                    .and_then(Value::as_f64)
                    .context("missing grass temperature")? as f32;
                let downfall = tint
                    .get("downfall")
                    .and_then(Value::as_f64)
                    .context("missing grass downfall")? as f32;
                ensure!(
                    temperature.is_finite() && downfall.is_finite(),
                    "invalid grass tint"
                );
                Ok(self
                    .grass
                    .as_ref()
                    .map_or(0x91bd5a, |map| map.lookup(temperature, downfall)))
            }
            Some("minecraft:constant" | "constant") => Ok(tint
                .get("value")
                .and_then(Value::as_i64)
                .context("unsupported constant tint")?
                as u32
                & 0xffffff),
            Some("minecraft:dye" | "dye") => Ok(key.dye.unwrap_or(
                tint.get("default")
                    .and_then(Value::as_i64)
                    .context("missing dye default")? as u32
                    & 0xffffff,
            )),
            _ => bail!("unsupported item tint"),
        }
    }
}

pub(crate) fn apply_palette(
    mut image: RgbaImage,
    source: &RgbaImage,
    target: &RgbaImage,
) -> Result<RgbaImage> {
    ensure!(source.len() == target.len(), "palette sizes do not match");
    let mut colors = HashMap::new();
    for (source, target) in source.pixels().zip(target.pixels()) {
        if source[3] != 0 {
            colors.insert([source[0], source[1], source[2]], target.0);
        }
    }
    for pixel in image.pixels_mut() {
        if pixel[3] == 0 {
            continue;
        }
        if let Some(target) = colors.get(&[pixel[0], pixel[1], pixel[2]]) {
            let alpha = (u16::from(pixel[3]) * u16::from(target[3]) / 255) as u8;
            pixel.0 = [target[0], target[1], target[2], alpha];
        }
    }
    Ok(image)
}

pub(crate) fn decode_png(bytes: Vec<u8>, maximum: u32) -> Result<RgbaImage> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(maximum);
    limits.max_image_height = Some(maximum);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?.to_rgba8())
}

enum Entry {
    Pending,
    Missing,
    Ready { model: Arc<HeldModel>, bytes: usize },
}

const MAX_CACHED_MODELS: usize = 64;
const MAX_CACHED_BYTES: usize = 128 * 1024 * 1024;
// Visible models may exceed the idle budget, but remain bounded even in a
// world with an unusually large number of distinct item models.
const MAX_ACTIVE_MODELS: usize = 256;
const MAX_ACTIVE_BYTES: usize = 256 * 1024 * 1024;
type LoadedItem = Option<(Arc<HeldModel>, usize)>;

// Count unique CPU images and quad storage once, on the background worker.
fn estimated_bytes(model: &HeldModel) -> usize {
    let mut textures = HashSet::new();
    let mut bytes = model
        .quads
        .len()
        .saturating_mul(std::mem::size_of::<TexturedQuad>());
    for quad in &model.quads {
        if textures.insert(Arc::as_ptr(&quad.texture)) {
            bytes = bytes.saturating_add(quad.texture.as_raw().len());
        }
    }
    bytes = bytes.saturating_add(
        model
            .gui_icon
            .as_ref()
            .map_or(0, |icon| icon.as_raw().len()),
    );
    bytes = bytes.saturating_add(model.gui_glint_quads.as_ref().map_or(0, |bounds| {
        bounds
            .len()
            .saturating_mul(std::mem::size_of::<Option<[f32; 4]>>())
    }));
    bytes = bytes.saturating_add(
        model
            .glint_materials
            .len()
            .saturating_mul(std::mem::size_of::<(Arc<RgbaImage>, Option<[f32; 4]>)>()),
    );
    bytes = bytes.saturating_add(
        model
            .gui_glint_pixels
            .as_ref()
            .map_or(0, |_| std::mem::size_of::<[[u16; 3]; 256]>()),
    );
    bytes.saturating_add(
        model
            .sprite_layers
            .len()
            .saturating_mul(std::mem::size_of::<SpriteLayer>()),
    )
}

pub struct ItemModelCache {
    request: mpsc::SyncSender<ModelKey>,
    response: mpsc::Receiver<(ModelKey, LoadedItem)>,
    entries: HashMap<ModelKey, Entry>,
    order: VecDeque<ModelKey>,
    bytes: usize,
    frame_started: bool,
    active: HashSet<ModelKey>,
    previous_active: HashSet<ModelKey>,
}

impl ItemModelCache {
    pub fn new(loader: Arc<Mutex<Loader>>) -> Self {
        let (request, incoming) = mpsc::sync_channel::<ModelKey>(32);
        let (outgoing, response) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(key) = incoming.recv() {
                let model = loader
                    .lock()
                    .ok()
                    .and_then(|mut loader| loader.load_visual(&key).ok())
                    .map(|mut model| {
                        if key.glint {
                            if let Some((icon, pixels)) = crate::item_gui::render_with_glint(&model)
                            {
                                model.gui_icon = Some(Arc::new(icon));
                                model.gui_glint_pixels = pixels;
                            }
                        } else {
                            model.gui_icon = crate::item_gui::render(&model).map(Arc::new);
                        }
                        // Only the 16×16 result is needed after rasterization.
                        model.gui_glint_quads = None;
                        let bytes = estimated_bytes(&model);
                        (Arc::new(model), bytes)
                    });
                if outgoing.send((key, model)).is_err() {
                    break;
                }
            }
        });
        Self {
            request,
            response,
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            frame_started: false,
            active: HashSet::new(),
            previous_active: HashSet::new(),
        }
    }

    /// Protect models used in this and the preceding frame. Eviction runs only
    /// after requests are recorded, so a large visible set can finish loading.
    pub fn begin_frame(&mut self) {
        self.frame_started = true;
        std::mem::swap(&mut self.previous_active, &mut self.active);
        self.active.clear();
    }

    /// Returns true when a completed request can change the visible preview.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for (id, model) in self.response.try_iter() {
            if let Some(entry) = self.entries.get_mut(&id) {
                if let Entry::Ready { bytes, .. } = entry {
                    self.bytes = self.bytes.saturating_sub(*bytes);
                }
                *entry = model.map_or(Entry::Missing, |(model, bytes)| {
                    self.bytes = self.bytes.saturating_add(bytes);
                    Entry::Ready { model, bytes }
                });
                changed = true;
            }
        }
        self.trim();
        changed
    }

    fn trim(&mut self) {
        while self.order.len() > MAX_CACHED_MODELS
            || (self.bytes > MAX_CACHED_BYTES && self.order.len() > 1)
        {
            let oldest = self.order.iter().position(|key| {
                !self.frame_started
                    || (!self.active.contains(key) && !self.previous_active.contains(key))
            });
            let hard_limit = self.order.len() > MAX_ACTIVE_MODELS
                || (self.bytes > MAX_ACTIVE_BYTES && self.order.len() > 1);
            let Some(index) = oldest.or_else(|| hard_limit.then_some(0)) else {
                break;
            };
            let Some(oldest) = self.order.remove(index) else {
                break;
            };
            if let Some(Entry::Ready { bytes, .. }) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(bytes);
            }
        }
    }

    pub fn get(&mut self, id: &str) -> Option<Arc<HeldModel>> {
        self.get_key(ModelKey::plain(id))
    }

    pub fn get_item(&mut self, item: &ItemStack) -> Option<Arc<HeldModel>> {
        self.get_key(ModelKey::from_item(item))
    }

    fn get_key(&mut self, key: ModelKey) -> Option<Arc<HeldModel>> {
        if let Some(entry) = self.entries.get(&key) {
            if self.frame_started {
                self.active.insert(key);
            }
            return match entry {
                Entry::Ready { model, .. } => Some(Arc::clone(model)),
                _ => None,
            };
        }
        if self.request.try_send(key.clone()).is_ok() {
            if self.frame_started {
                self.active.insert(key.clone());
            }
            self.entries.insert(key.clone(), Entry::Pending);
            self.order.push_back(key);
            self.trim();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn select_uses_trim_material_and_palette_preserves_alpha() {
        let model = serde_json::json!({"type":"minecraft:select","property":"minecraft:trim_material","cases":[{"when":["minecraft:quartz","minecraft:iron"],"model":{"type":"minecraft:model","model":"minecraft:item/trimmed"}}],"fallback":{"type":"minecraft:model","model":"minecraft:item/plain"}});
        let mut key = ModelKey::plain("minecraft:iron_helmet");
        assert_eq!(
            selected_model(&model, &key).unwrap()["model"],
            "minecraft:item/plain"
        );
        key.trim_material = Some("minecraft:quartz".into());
        assert_eq!(
            selected_model(&model, &key).unwrap()["model"],
            "minecraft:item/trimmed"
        );
        key.trim_material = Some("example:unknown".into());
        assert_eq!(
            selected_model(&model, &key).unwrap()["model"],
            "minecraft:item/plain"
        );
        let source = RgbaImage::from_raw(2, 1, vec![10, 20, 30, 255, 40, 50, 60, 0]).unwrap();
        let target =
            RgbaImage::from_raw(2, 1, vec![100, 110, 120, 128, 200, 210, 220, 255]).unwrap();
        let input =
            RgbaImage::from_raw(3, 1, vec![10, 20, 30, 128, 40, 50, 60, 255, 10, 20, 30, 0])
                .unwrap();
        let output = apply_palette(input, &source, &target).unwrap();
        assert_eq!(output.get_pixel(0, 0).0, [100, 110, 120, 64]);
        assert_eq!(output.get_pixel(1, 0).0, [40, 50, 60, 255]);
        assert_eq!(output.get_pixel(2, 0).0, [10, 20, 30, 0]);
    }

    #[test]
    fn model_cache_separates_trim_and_dye_for_the_same_item() {
        let item = |material: &str, dye: i32| {
            ItemStack::new(
                "minecraft:leather_helmet".into(),
                1,
                1,
                Tag::Compound(vec![
                    (
                        "minecraft:trim".into(),
                        Tag::Compound(vec![("material".into(), Tag::String(material.into()))]),
                    ),
                    ("minecraft:dyed_color".into(), Tag::Int(dye)),
                ]),
            )
            .unwrap()
        };
        let first = ModelKey::from_item(&item("quartz", 0x123456));
        let second = ModelKey::from_item(&item("minecraft:iron", 0x123456));
        let third = ModelKey::from_item(&item("minecraft:quartz", 0x654321));
        assert_eq!(first.trim_material.as_deref(), Some("minecraft:quartz"));
        assert_ne!(first, second);
        assert_ne!(first, third);
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn palette_mapping_matches_java_for_transparency_and_duplicate_colors() {
        use std::process::Command;
        const JAVA: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.client.resources.palette.Palette;
import net.minecraft.client.resources.palette.PaletteMapping;
public class PaletteOracle {
  public static void main(String[] args) throws Exception {
    var data=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
    int[] from=new int[data.getAsJsonArray("from").size()], to=new int[from.length];
    for(int i=0;i<from.length;i++){from[i]=data.getAsJsonArray("from").get(i).getAsInt();to[i]=data.getAsJsonArray("to").get(i).getAsInt();}
    var mapping=PaletteMapping.create(Palette.of(from),Palette.of(to)); var out=new JsonArray();
    for(var value:data.getAsJsonArray("input"))out.add(mapping.apply(value.getAsInt()));
    Files.writeString(Path.of(args[1]),new Gson().toJson(out));
  }
}
"#;
        let source = [0xff112233_u32, 0x00445566, 0x80112233, 0xffabcdef];
        let target = [0xff102030_u32, 0xff405060, 0x80405060, 0x00010203];
        let mut pixels = Vec::new();
        for alpha in 0..=255_u32 {
            for color in [0x112233, 0x445566, 0xabcdef, 0x928374] {
                pixels.push((alpha << 24) | color);
            }
        }
        let signed = |values: &[u32]| values.iter().map(|value| *value as i32).collect::<Vec<_>>();
        let temp = tempfile::tempdir().unwrap();
        let java_source = temp.path().join("PaletteOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&java_source, JAVA).unwrap();
        std::fs::write(&input, serde_json::to_vec(&serde_json::json!({"from":signed(&source),"to":signed(&target),"input":signed(&pixels)})).unwrap()).unwrap();
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = ResourceStack::open(&game).unwrap();
        let version = game.join("versions/26.3");
        let metadata: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java palette oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&java_source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("PaletteOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<i32> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let image = |values: &[u32]| {
            RgbaImage::from_fn(values.len() as u32, 1, |x, _| {
                let value = values[x as usize];
                image::Rgba([
                    (value >> 16) as u8,
                    (value >> 8) as u8,
                    value as u8,
                    (value >> 24) as u8,
                ])
            })
        };
        let actual = apply_palette(image(&pixels), &image(&source), &image(&target)).unwrap();
        for (index, (pixel, expected)) in actual.pixels().zip(expected).enumerate() {
            let argb = (u32::from(pixel[3]) << 24)
                | (u32::from(pixel[0]) << 16)
                | (u32::from(pixel[1]) << 8)
                | u32::from(pixel[2]);
            assert_eq!(argb as i32, expected, "palette sample {index}");
        }
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn armor_models_use_trim_palettes_and_per_item_dye() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut loader = open_loader(&game, &[]);
        let plain = loader
            .load_visual(&ModelKey::plain("minecraft:iron_helmet"))
            .unwrap();
        assert_eq!(plain.sprite_layers.len(), 1);
        let mut trimmed = ModelKey::plain("minecraft:iron_helmet");
        trimmed.trim_material = Some("minecraft:quartz".into());
        let trimmed = loader.load_visual(&trimmed).unwrap();
        assert_eq!(trimmed.sprite_layers.len(), 2);
        assert!(
            trimmed.sprite_layers[1]
                .texture
                .pixels()
                .any(|pixel| pixel[3] > 0)
        );
        let temp = tempfile::tempdir().unwrap();
        let palette_path = "assets/minecraft/textures/palettes/trim/quartz.png";
        let mut replacement = decode_png(
            loader.stack.read_asset(palette_path).unwrap().unwrap(),
            4096,
        )
        .unwrap();
        for pixel in replacement.pixels_mut() {
            if pixel[3] != 0 {
                pixel[0] = 240;
                pixel[1] = 5;
                pixel[2] = 200;
            }
        }
        let destination = temp.path().join(palette_path);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        replacement.save(destination).unwrap();
        let mut packed = open_loader(&game, &[temp.path().to_path_buf()]);
        let mut key = ModelKey::plain("minecraft:iron_helmet");
        key.trim_material = Some("minecraft:quartz".into());
        let overridden = packed.load_visual(&key).unwrap();
        assert_ne!(
            trimmed.sprite_layers[1].texture.as_raw(),
            overridden.sprite_layers[1].texture.as_raw()
        );
        let leather = loader
            .load_visual(&ModelKey::plain("minecraft:leather_helmet"))
            .unwrap();
        assert_eq!(leather.sprite_layers.len(), 2);
        let mut dyed = ModelKey::plain("minecraft:leather_helmet");
        dyed.dye = Some(0x123456);
        let dyed = loader.load_visual(&dyed).unwrap();
        assert_ne!(dyed.sprite_layers[0].tint, leather.sprite_layers[0].tint);
        assert_eq!(
            dyed.sprite_layers[0].tint,
            [18.0 / 255.0, 52.0 / 255.0, 86.0 / 255.0]
        );
        assert_eq!(dyed.sprite_layers[1].tint, [1.0; 3]);
    }

    fn open_loader(game: &std::path::Path, packs: &[PathBuf]) -> Loader {
        let mut stack = ResourceStack::open(game).unwrap();
        for path in packs {
            stack.push_pack(path).unwrap();
        }
        Loader::from_stack(stack)
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn glint_models_use_the_correct_atlases_without_loading_them_for_plain_items() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut loader = open_loader(&game, &[]);
        loader
            .load_visual(&ModelKey::plain("minecraft:diamond_sword"))
            .unwrap();
        assert!(loader.item_atlas.is_none() && loader.block_atlas.is_none());

        let mut sword = ModelKey::plain("minecraft:diamond_sword");
        sword.glint = true;
        let sword = loader.load_visual(&sword).unwrap();
        assert_eq!(
            sword.numeric_item_id,
            loader
                .item_registry
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .id("minecraft:diamond_sword")
        );
        assert!(sword.numeric_item_id.is_some());
        assert_eq!(sword.ground.translation, Vec3::new(0.0, 0.125, 0.0));
        assert_eq!(sword.ground.scale, Vec3::splat(0.5));
        assert!(sword.gui_glint_uv.is_some());
        assert!(sword.glint && sword.glint_materials.len() == 1);
        assert_eq!(sword.glint_materials[0].1, sword.gui_glint_uv);
        assert!(loader.item_atlas.as_ref().is_some_and(Option::is_some));
        assert!(loader.block_atlas.is_none());

        let mut stone = ModelKey::plain("minecraft:stone");
        stone.glint = true;
        let stone = loader.load_visual(&stone).unwrap();
        assert!(stone.numeric_item_id.is_some());
        assert_eq!(stone.ground.translation, Vec3::new(0.0, 3.0 / 16.0, 0.0));
        assert_eq!(stone.ground.scale, Vec3::splat(0.25));
        assert!(stone.glint_materials.iter().all(|(_, uv)| uv.is_some()));
        assert!(
            stone
                .gui_glint_quads
                .as_ref()
                .unwrap()
                .iter()
                .any(Option::is_some)
        );
        assert!(loader.block_atlas.as_ref().is_some_and(Option::is_some));
        let (_, pixels) = crate::item_gui::render_with_glint(&stone).unwrap();
        assert!(pixels.unwrap().iter().any(|pixel| pixel[2] == 1));

        let mut helmet = ModelKey::plain("minecraft:iron_helmet");
        helmet.glint = true;
        helmet.trim_material = Some("minecraft:quartz".into());
        let helmet = loader.load_visual(&helmet).unwrap();
        assert_eq!(helmet.sprite_layers.len(), 2);
        assert_eq!(helmet.glint_materials.len(), 2);
        assert!(
            helmet
                .sprite_layers
                .iter()
                .all(|layer| layer.glint_uv.is_some())
        );
        for layer in &helmet.sprite_layers {
            assert!(
                helmet
                    .glint_materials
                    .iter()
                    .any(|(texture, bounds)| Arc::ptr_eq(texture, &layer.texture)
                        && *bounds == layer.glint_uv)
            );
        }
        let (_, pixels) = crate::item_gui::render_with_glint(&helmet).unwrap();
        let pixels = pixels.unwrap();
        for layer in &helmet.sprite_layers {
            let [u0, v0, u1, v1] = layer.glint_uv.unwrap();
            assert!(
                pixels.iter().any(|pixel| {
                    let u = f32::from(pixel[0]) / 65535.0;
                    let v = f32::from(pixel[1]) / 65535.0;
                    pixel[2] == 1 && (u0..=u1).contains(&u) && (v0..=v1).contains(&v)
                }),
                "a visible pixel should use each layer's atlas coordinates"
            );
        }

        let pack = tempfile::tempdir().unwrap();
        let definition = pack.path().join("assets/minecraft/atlases/items.json");
        std::fs::create_dir_all(definition.parent().unwrap()).unwrap();
        std::fs::write(
            &definition,
            br#"{"sources":[{"type":"minecraft:single","resource":"minecraft:block/stone"}]}"#,
        )
        .unwrap();
        let custom_model = pack
            .path()
            .join("assets/minecraft/models/item/diamond_sword.json");
        std::fs::create_dir_all(custom_model.parent().unwrap()).unwrap();
        std::fs::write(&custom_model, br#"{"parent":"minecraft:item/handheld","textures":{"layer0":"minecraft:item/diamond_sword"},"display":{"ground":{"rotation":[15,30,0],"translation":[4,8,-2],"scale":[0.3,0.4,0.5]}}}"#).unwrap();
        let mut packed = open_loader(&game, &[pack.path().to_path_buf()]);
        let custom = packed
            .load_visual(&ModelKey::plain("minecraft:diamond_sword"))
            .unwrap();
        assert_eq!(custom.ground.translation, Vec3::new(0.25, 0.5, -0.125));
        assert_eq!(custom.ground.rotation, Vec3::new(15.0, 30.0, 0.0));
        assert_eq!(custom.ground.scale, Vec3::new(0.3, 0.4, 0.5));
        let sprite = ResourceId::parse("minecraft:block/stone").unwrap();
        let uv = packed.glint_uv(&sprite).unwrap();
        let atlas = packed.item_atlas.as_ref().unwrap().as_ref().unwrap();
        let position = &atlas.placements[&sprite];
        assert_eq!(
            uv[0],
            (position.x + position.padding) as f32 / atlas.width as f32
        );
        assert!(packed.block_atlas.is_none(), "item atlas takes precedence");
    }

    #[test]
    fn display_transform_uses_vanilla_defaults_and_limits() {
        let value =
            serde_json::json!({"rotation":[75,45,10],"translation":[96,-96,8],"scale":[5,-5,0.5]});
        let transform = DisplayTransform::parse(Some(&value)).unwrap();
        assert_eq!(transform.translation, Vec3::new(5.0, -5.0, 0.5));
        assert_eq!(transform.scale, Vec3::new(4.0, -4.0, 0.5));
        assert_eq!(transform.rotation, Vec3::new(75.0, 45.0, 10.0));
        assert!(DisplayTransform::parse(Some(&serde_json::json!({"scale":[1,2]}))).is_err());
        assert_eq!(DisplayTransform::parse(None).unwrap().scale, Vec3::ONE);
    }

    fn test_model() -> Arc<HeldModel> {
        let display = DisplayTransform::parse(None).unwrap();
        let image = Arc::new(RgbaImage::new(1, 1));
        Arc::new(HeldModel {
            numeric_item_id: None,
            quads: crate::generated_item::bake(Arc::clone(&image), [1.0; 3], 0).unwrap(),
            glint: false,
            glint_materials: Vec::new(),
            right: display,
            left: display,
            first_right: display,
            first_left: display,
            ground: display,
            gui: display,
            gui_side_light: false,
            gui_icon: None,
            gui_glint_uv: None,
            gui_glint_pixels: None,
            gui_glint_quads: None,
            sprite_layers: vec![SpriteLayer {
                texture: image,
                tint: [1.0; 3],
                glint_uv: None,
            }],
        })
    }

    #[test]
    fn cache_retains_a_full_inventory_and_evicts_when_memory_is_full() {
        let (request, requests) = mpsc::sync_channel(64);
        let (results, response) = mpsc::channel();
        let mut cache = ItemModelCache {
            request,
            response,
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            frame_started: false,
            active: HashSet::new(),
            previous_active: HashSet::new(),
        };
        let model = test_model();
        let size = estimated_bytes(&model);
        for index in 0..45 {
            let id = format!("minecraft:item_{index}");
            assert!(cache.get(&id).is_none());
            let key = ModelKey::plain(&id);
            assert_eq!(requests.try_recv().unwrap(), key);
            results
                .send((key, Some((Arc::clone(&model), size))))
                .unwrap();
        }
        assert!(cache.poll());
        for index in 0..45 {
            assert!(cache.get(&format!("minecraft:item_{index}")).is_some());
        }

        // Model sizes come from the worker. Simulate large textures without
        // allocating hundreds of MiB, then check old models are evicted.
        let large_size = MAX_CACHED_BYTES / 32;
        for index in 0..45 {
            results
                .send((
                    ModelKey::plain(&format!("minecraft:item_{index}")),
                    Some((Arc::clone(&model), large_size)),
                ))
                .unwrap();
        }
        assert!(cache.poll());
        assert!(cache.bytes <= MAX_CACHED_BYTES);
        assert!(
            !cache
                .entries
                .contains_key(&ModelKey::plain("minecraft:item_0"))
        );
        assert!(cache.get("minecraft:item_44").is_some());
    }

    #[test]
    fn cache_keeps_large_visible_set_and_reclaims_it_after_use() {
        let (baseline_request, baseline_requests) = mpsc::sync_channel(128);
        let (baseline_results, baseline_response) = mpsc::channel();
        let mut baseline = ItemModelCache {
            request: baseline_request,
            response: baseline_response,
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            frame_started: false,
            active: HashSet::new(),
            previous_active: HashSet::new(),
        };
        let model = test_model();
        let size = estimated_bytes(&model);
        for index in 0..80 {
            baseline.get(&format!("minecraft:item_{index}"));
        }
        for _ in 0..80 {
            baseline_results
                .send((
                    baseline_requests.try_recv().unwrap(),
                    Some((Arc::clone(&model), size)),
                ))
                .unwrap();
        }
        baseline.poll();
        let baseline_hits = (0..80)
            .filter(|index| baseline.get(&format!("minecraft:item_{index}")).is_some())
            .count();
        assert_eq!(
            baseline_hits, 0,
            "FIFO eviction reproduces the visible-set churn"
        );

        let (request, requests) = mpsc::sync_channel(128);
        let (results, response) = mpsc::channel();
        let mut cache = ItemModelCache {
            request,
            response,
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            frame_started: false,
            active: HashSet::new(),
            previous_active: HashSet::new(),
        };
        cache.begin_frame();
        for index in 0..80 {
            assert!(cache.get(&format!("minecraft:item_{index}")).is_none());
        }
        for _ in 0..80 {
            results
                .send((
                    requests.try_recv().unwrap(),
                    Some((Arc::clone(&model), size)),
                ))
                .unwrap();
        }
        assert!(cache.poll());
        cache.begin_frame();
        for index in 0..80 {
            assert!(
                cache.get(&format!("minecraft:item_{index}")).is_some(),
                "visible item {index} was evicted"
            );
        }
        eprintln!(
            "80 visible models: {baseline_hits} cache hits with FIFO, 80 with frame protection"
        );
        assert!(
            requests.try_recv().is_err(),
            "visible items must not reload every frame"
        );
        cache.begin_frame();
        cache.begin_frame();
        cache.poll();
        assert!(cache.entries.len() <= MAX_CACHED_MODELS);
    }

    #[test]
    fn visible_models_still_obey_hard_cache_limits() {
        let (request, _requests) = mpsc::sync_channel(512);
        let (results, response) = mpsc::channel();
        let mut cache = ItemModelCache {
            request,
            response,
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            frame_started: false,
            active: HashSet::new(),
            previous_active: HashSet::new(),
        };
        cache.begin_frame();
        for index in 0..(MAX_ACTIVE_MODELS + 40) {
            cache.get(&format!("minecraft:item_{index}"));
        }
        assert_eq!(cache.entries.len(), MAX_ACTIVE_MODELS);
        let model = test_model();
        for index in 40..43 {
            results
                .send((
                    ModelKey::plain(&format!("minecraft:item_{index}")),
                    Some((Arc::clone(&model), MAX_ACTIVE_BYTES / 2)),
                ))
                .unwrap();
        }
        cache.poll();
        assert!(cache.bytes <= MAX_ACTIVE_BYTES);
        assert!(cache.entries.len() <= MAX_ACTIVE_MODELS);
    }

    #[test]
    fn visible_models_finish_loading_through_bounded_worker_queue() {
        let (request, requests) = mpsc::sync_channel(32);
        let (results, response) = mpsc::channel();
        let mut cache = ItemModelCache {
            request,
            response,
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            frame_started: false,
            active: HashSet::new(),
            previous_active: HashSet::new(),
        };
        let model = test_model();
        let size = estimated_bytes(&model);
        let mut hits = 0;
        for _ in 0..8 {
            cache.begin_frame();
            cache.poll();
            hits = (0..80)
                .filter(|index| cache.get(&format!("minecraft:item_{index}")).is_some())
                .count();
            while let Ok(key) = requests.try_recv() {
                results
                    .send((key, Some((Arc::clone(&model), size))))
                    .unwrap();
            }
        }
        assert_eq!(hits, 80);
        assert!(requests.try_recv().is_err());
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn local_models_and_pack_override_load_without_blocking_rendering() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut loader = open_loader(&game, &[]);
        for id in [
            "minecraft:stone",
            "minecraft:dirt",
            "minecraft:grass_block",
            "minecraft:stone_bricks",
            "minecraft:stone_button",
        ] {
            let model = loader
                .load(id)
                .unwrap_or_else(|error| panic!("{id}: {error:#}"));
            assert!(!model.quads.is_empty());
            assert_eq!(model.right.rotation, Vec3::new(75.0, 45.0, 0.0));
            assert_eq!(model.left.rotation, model.right.rotation);
            assert_eq!(model.first_right.scale, Vec3::splat(0.4));
            assert_eq!(model.first_right.rotation, Vec3::new(0.0, 45.0, 0.0));
            assert_eq!(model.first_left.rotation, Vec3::new(0.0, 225.0, 0.0));
        }
        let grass = loader.load("minecraft:grass_block").unwrap();
        assert!(grass.quads.iter().any(|quad| quad.tint != [1.0; 3]));
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("assets/minecraft");
        std::fs::create_dir_all(base.join("items")).unwrap();
        std::fs::create_dir_all(base.join("models/block")).unwrap();
        std::fs::create_dir_all(base.join("textures/block")).unwrap();
        std::fs::write(
            base.join("items/stone.json"),
            r#"{"model":{"type":"minecraft:model","model":"minecraft:block/test_hand"}}"#,
        )
        .unwrap();
        std::fs::write(base.join("models/block/test_hand.json"), r#"{"parent":"minecraft:block/button_inventory","textures":{"texture":"minecraft:block/test_hand"},"display":{"thirdperson_righthand":{"rotation":[7,13,19]},"firstperson_righthand":{"rotation":[3,7,11]}}}"#).unwrap();
        RgbaImage::from_pixel(16, 16, image::Rgba([9, 63, 127, 255]))
            .save(base.join("textures/block/test_hand.png"))
            .unwrap();
        let mut loader = open_loader(&game, &[temp.path().to_path_buf()]);
        let model = loader.load("minecraft:stone").unwrap();
        assert_eq!(model.right.rotation, Vec3::new(7.0, 13.0, 19.0));
        assert_eq!(model.left.rotation, model.right.rotation);
        assert_eq!(model.first_right.rotation, Vec3::new(3.0, 7.0, 11.0));
        assert_eq!(model.first_left.rotation, model.first_right.rotation);
        assert_eq!(model.quads[0].texture.get_pixel(0, 0).0, [9, 63, 127, 255]);

        let mut cache = ItemModelCache::new(Arc::new(Mutex::new(loader)));
        assert!(cache.get("minecraft:stone").is_none());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let model = loop {
            cache.poll();
            if let Some(model) = cache.get("minecraft:stone") {
                break model;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background item load did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(model.right.rotation, Vec3::new(7.0, 13.0, 19.0));
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn all_button_item_models_load_from_installed_resources() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut loader = open_loader(&game, &[]);
        for button in native_core::button::Button::ALL {
            let model = loader
                .load(button.id())
                .unwrap_or_else(|error| panic!("{}: {error:#}", button.id()));
            assert!(!model.quads.is_empty(), "{}", button.id());
            assert!(crate::item_gui::render(&model).is_some(), "{}", button.id());
        }
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn generated_items_and_layered_pack_overrides_load() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut loader = open_loader(&game, &[]);
        for id in [
            "minecraft:diamond_sword",
            "minecraft:iron_pickaxe",
            "minecraft:apple",
        ] {
            let model = loader
                .load(id)
                .unwrap_or_else(|error| panic!("{id}: {error:#}"));
            assert_eq!(model.sprite_layers.len(), 1);
            assert!(model.quads.len() > 2);
            assert_eq!(model.gui.rotation, Vec3::ZERO);
            assert_eq!(model.first_right.rotation, Vec3::new(0.0, -90.0, 25.0));
        }
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("assets/minecraft");
        std::fs::create_dir_all(base.join("items")).unwrap();
        std::fs::create_dir_all(base.join("models/item")).unwrap();
        std::fs::create_dir_all(base.join("textures/item")).unwrap();
        std::fs::write(base.join("items/apple.json"),r#"{"model":{"type":"minecraft:model","model":"minecraft:item/test_layered","tints":[{"type":"minecraft:constant","value":16711680},{"type":"minecraft:constant","value":65280}]}}"#).unwrap();
        std::fs::write(base.join("models/item/test_layered.json"),r#"{"parent":"minecraft:item/generated","textures":{"layer0":"minecraft:item/test_a","layer1":"minecraft:item/test_b"}}"#).unwrap();
        RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 255]))
            .save(base.join("textures/item/test_a.png"))
            .unwrap();
        RgbaImage::from_pixel(2, 2, image::Rgba([255, 255, 255, 128]))
            .save(base.join("textures/item/test_b.png"))
            .unwrap();
        let model = open_loader(&game, &[temp.path().to_path_buf()])
            .load("minecraft:apple")
            .unwrap();
        assert_eq!(model.sprite_layers.len(), 2);
        assert_eq!(model.sprite_layers[0].tint, [1.0, 0.0, 0.0]);
        assert_eq!(model.sprite_layers[1].tint, [0.0, 1.0, 0.0]);
        assert!(model.quads.iter().any(|quad| quad.layer == 0));
        assert!(model.quads.iter().any(|quad| quad.layer == 1));
    }
}
