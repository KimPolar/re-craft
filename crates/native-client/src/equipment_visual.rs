//! Resolve humanoid equipment textures without doing resource I/O on the UI thread.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, mpsc},
    thread,
};

use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use native_persistence::nbt::Tag;
use native_resources::packs::{ResourceId, ResourceProvider};
use native_session::inventory::ItemStack;
use serde_json::Value;

use crate::held_items::{Loader, apply_palette, decode_png};

fn canonical(id: &str) -> Option<String> {
    let id = ResourceId::parse(id).ok()?;
    Some(format!("{}:{}", id.namespace, id.path))
}

fn text(tag: &Tag) -> Option<String> {
    match tag {
        Tag::String(value) => String::from_utf16(&value.0).ok(),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct Pattern {
    asset: String,
    decal: bool,
}

/// Trim IDs are world registry entries, so they must come from the active data packs.
#[derive(Clone, Debug, Default)]
pub struct TrimRegistry {
    materials: HashMap<String, String>,
    patterns: HashMap<String, Pattern>,
}

impl TrimRegistry {
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let mut registry = Self::default();
        for folder in ["trim_material", "trim_pattern"] {
            for id in provider.list_data(folder)? {
                let path = format!("data/{}/{folder}/{}.json", id.namespace, id.path);
                let Some(bytes) = provider.read_data(&path)? else {
                    continue;
                };
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                let key = format!("{}:{}", id.namespace, id.path);
                if folder == "trim_material" {
                    if let Some(palette) = value
                        .get("palette_id")
                        .and_then(Value::as_str)
                        .and_then(canonical)
                    {
                        registry.materials.insert(key, palette);
                    }
                } else if let Some(asset) = value
                    .get("asset_id")
                    .and_then(Value::as_str)
                    .and_then(canonical)
                {
                    registry.patterns.insert(
                        key,
                        Pattern {
                            asset,
                            decal: value.get("decal").and_then(Value::as_bool).unwrap_or(false),
                        },
                    );
                }
            }
        }
        Ok(registry)
    }

    fn resolve(&self, tag: &Tag) -> Option<TrimKey> {
        let material = canonical(&text(tag.get("material")?)?)?;
        let pattern = canonical(&text(tag.get("pattern")?)?)?;
        let palette = self.materials.get(&material)?.clone();
        let info = self.patterns.get(&pattern)?;
        Some(TrimKey {
            material,
            pattern,
            asset: info.asset.clone(),
            palette,
            decal: info.decal,
        })
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TrimKey {
    material: String,
    pattern: String,
    asset: String,
    palette: String,
    decal: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ArmorKey {
    asset: String,
    leggings: bool,
    dye: Option<u32>,
    trim: Option<TrimKey>,
}

fn default_asset(item: &str, slot: usize) -> Option<&'static str> {
    if item == "minecraft:turtle_helmet" && slot == 0 {
        return Some("minecraft:turtle_scute");
    }
    let name = item.strip_prefix("minecraft:")?;
    let (material, piece) = name.split_once('_')?;
    if ["helmet", "chestplate", "leggings", "boots"].get(slot)? != &piece {
        return None;
    }
    match material {
        "leather" => Some("minecraft:leather"),
        "golden" => Some("minecraft:gold"),
        "copper" => Some("minecraft:copper"),
        "chainmail" => Some("minecraft:chainmail"),
        "iron" => Some("minecraft:iron"),
        "diamond" => Some("minecraft:diamond"),
        "netherite" => Some("minecraft:netherite"),
        _ => None,
    }
}

impl ArmorKey {
    fn from_item(item: &ItemStack, slot: usize, trims: &TrimRegistry) -> Option<Self> {
        let components = item.components();
        if components.get("!minecraft:equippable").is_some() {
            return None;
        }
        let asset = if let Some(equippable) = components.get("minecraft:equippable") {
            let expected = ["head", "chest", "legs", "feet"].get(slot)?;
            if text(equippable.get("slot")?).as_deref() != Some(expected) {
                return None;
            }
            canonical(&text(equippable.get("asset_id")?)?)?
        } else {
            default_asset(item.id(), slot)?.to_owned()
        };
        let dye = components
            .get("minecraft:dyed_color")
            .and_then(Tag::as_java_int)
            .map(|value| value as u32 & 0x00ff_ffff);
        let trim = components
            .get("minecraft:trim")
            .and_then(|tag| trims.resolve(tag));
        Some(Self {
            asset,
            leggings: slot == 2,
            dye,
            trim,
        })
    }
}

pub struct ArmorLayer {
    pub texture: Arc<RgbaImage>,
    pub tint: [f32; 3],
}

pub struct ArmorAppearance {
    pub layers: Vec<ArmorLayer>,
}

fn rgb(color: u32) -> [f32; 3] {
    [
        ((color >> 16) & 255) as f32 / 255.0,
        ((color >> 8) & 255) as f32 / 255.0,
        (color & 255) as f32 / 255.0,
    ]
}

fn read_image(provider: &mut impl ResourceProvider, path: &str) -> Result<RgbaImage> {
    let asset = provider
        .read_asset_with_metadata(path)?
        .with_context(|| format!("missing {path}"))?;
    let image = decode_png(asset.bytes, 4096)?;
    native_resources::sprites::first_frame(&image, asset.metadata.as_deref())
}

fn trim_image(
    provider: &mut impl ResourceProvider,
    key: &TrimKey,
    layer: &str,
    definition: &Value,
) -> Result<RgbaImage> {
    let mut asset = key.asset.clone();
    let mut palette = Some(key.palette.clone());
    if let Some(overrides) = definition.get("trim_overrides").and_then(Value::as_array) {
        for entry in overrides {
            let Some(predicate) = entry.get("when").and_then(Value::as_object) else {
                continue;
            };
            let matches = |field: &str, expected: &str| {
                predicate.get(field).is_none_or(|value| {
                    value.as_str().and_then(canonical).as_deref() == Some(expected)
                })
            };
            if !matches("material", &key.material) || !matches("pattern", &key.pattern) {
                continue;
            }
            if let Some(value) = entry
                .get("texture")
                .and_then(Value::as_str)
                .and_then(canonical)
            {
                asset = value;
            }
            palette = entry
                .get("palette")
                .and_then(Value::as_str)
                .and_then(canonical);
            break;
        }
    }
    let asset = ResourceId::parse(&asset)?;
    let path = format!(
        "assets/{}/textures/trims/entity/{layer}/{}.png",
        asset.namespace, asset.path
    );
    let texture = provider
        .read_asset_with_metadata(&path)?
        .with_context(|| format!("missing {path}"))?;
    let image = decode_png(texture.bytes, 4096)?;
    let metadata: Option<Value> = texture
        .metadata
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()?;
    let Some(palette) = palette else {
        return Ok(image);
    };
    let base = metadata
        .as_ref()
        .and_then(|v| v.get("palette"))
        .and_then(|v| v.get("base_palette"))
        .and_then(Value::as_str)
        .context("trim texture has no base palette")?;
    let source = ResourceId::parse(base)?.asset_path("textures/palettes", "png");
    let target = ResourceId::parse(&palette)?.asset_path("textures/palettes", "png");
    let source = read_image(provider, &source)?;
    let target = read_image(provider, &target)?;
    apply_palette(image, &source, &target)
}

pub(crate) fn load(
    provider: &mut impl ResourceProvider,
    key: &ArmorKey,
) -> Result<ArmorAppearance> {
    let asset = ResourceId::parse(&key.asset)?;
    let path = asset.asset_path("equipment", "json");
    let bytes = provider
        .read_asset(&path)?
        .with_context(|| format!("missing {path}"))?;
    let definition: Value = serde_json::from_slice(&bytes)?;
    let layer_type = if key.leggings {
        "humanoid_leggings"
    } else {
        "humanoid"
    };
    let layers = definition
        .get("layers")
        .and_then(|v| v.get(layer_type))
        .and_then(Value::as_array)
        .context("equipment has no humanoid layers")?;
    ensure!(layers.len() <= 32, "too many equipment layers");
    let mut result = Vec::new();
    let mut bytes = 0usize;
    for layer in layers {
        let tint = if let Some(dyeable) = layer.get("dyeable") {
            let color = key.dye.or_else(|| {
                dyeable
                    .get("color_when_undyed")
                    .and_then(Value::as_i64)
                    .map(|v| v as u32 & 0xffffff)
            });
            let Some(color) = color else { continue };
            rgb(color)
        } else {
            [1.0; 3]
        };
        let texture = layer
            .get("texture")
            .and_then(Value::as_str)
            .context("equipment layer has no texture")?;
        let texture = ResourceId::parse(texture)?;
        let path = format!(
            "assets/{}/textures/entity/equipment/{layer_type}/{}.png",
            texture.namespace, texture.path
        );
        let texture = read_image(provider, &path)?;
        bytes = bytes.saturating_add(texture.as_raw().len());
        ensure!(bytes <= MAX_BYTES, "equipment textures exceed cache budget");
        result.push(ArmorLayer {
            texture: Arc::new(texture),
            tint,
        });
    }
    if !result.is_empty()
        && let Some(trim) = &key.trim
    {
        match trim_image(provider, trim, layer_type, &definition) {
            Ok(texture) if bytes.saturating_add(texture.as_raw().len()) <= MAX_BYTES => {
                result.push(ArmorLayer {
                    texture: Arc::new(texture),
                    tint: [1.0; 3],
                });
            }
            Ok(_) => eprintln!("armor trim {}: texture exceeds cache budget", key.asset),
            Err(error) => eprintln!("armor trim {}: {error:#}", key.asset),
        }
    }
    Ok(ArmorAppearance { layers: result })
}

enum Entry {
    Pending,
    Missing,
    Ready {
        appearance: Arc<ArmorAppearance>,
        bytes: usize,
    },
}
const MAX_ENTRIES: usize = 64;
const MAX_BYTES: usize = 128 * 1024 * 1024;

pub struct ArmorCache {
    request: mpsc::SyncSender<ArmorKey>,
    response: mpsc::Receiver<(ArmorKey, Option<Arc<ArmorAppearance>>)>,
    entries: HashMap<ArmorKey, Entry>,
    order: VecDeque<ArmorKey>,
    bytes: usize,
    trims: Arc<TrimRegistry>,
}

impl ArmorCache {
    pub fn new(loader: Arc<Mutex<Loader>>, trims: Arc<TrimRegistry>) -> Self {
        let (request, incoming) = mpsc::sync_channel::<ArmorKey>(32);
        let (outgoing, response) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(key) = incoming.recv() {
                let appearance = loader
                    .lock()
                    .ok()
                    .and_then(|mut loader| loader.load_armor(&key).ok())
                    .map(Arc::new);
                if outgoing.send((key, appearance)).is_err() {
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
            trims,
        }
    }

    pub fn set_trims(&mut self, trims: Arc<TrimRegistry>) {
        self.trims = trims;
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for (key, appearance) in self.response.try_iter() {
            if let Some(entry) = self.entries.get_mut(&key) {
                if let Entry::Ready { bytes, .. } = entry {
                    self.bytes = self.bytes.saturating_sub(*bytes);
                }
                *entry = appearance.map_or(Entry::Missing, |appearance| {
                    let bytes = appearance
                        .layers
                        .iter()
                        .map(|layer| layer.texture.as_raw().len())
                        .sum::<usize>();
                    self.bytes = self.bytes.saturating_add(bytes);
                    Entry::Ready { appearance, bytes }
                });
                changed = true;
            }
        }
        self.trim();
        changed
    }

    fn trim(&mut self) {
        while self.order.len() > MAX_ENTRIES || (self.bytes > MAX_BYTES && self.order.len() > 1) {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(Entry::Ready { bytes, .. }) = self.entries.remove(&old) {
                self.bytes = self.bytes.saturating_sub(bytes);
            }
        }
    }

    pub fn get(&mut self, item: &ItemStack, slot: usize) -> Option<Arc<ArmorAppearance>> {
        let key = ArmorKey::from_item(item, slot, &self.trims)?;
        if let Some(entry) = self.entries.get(&key) {
            return match entry {
                Entry::Ready { appearance, .. } => Some(Arc::clone(appearance)),
                _ => None,
            };
        }
        if self.request.try_send(key.clone()).is_ok() {
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

    fn stack(id: &str, components: Vec<(&str, Tag)>) -> ItemStack {
        ItemStack::unresolved(
            id.into(),
            1,
            Tag::Compound(components.into_iter().map(|(k, v)| (k.into(), v)).collect()),
        )
        .unwrap()
    }

    fn string(value: &str) -> Tag {
        Tag::String(value.into())
    }

    #[test]
    fn equipment_components_select_the_correct_asset_slot_and_dye() {
        let trims = TrimRegistry::default();
        let iron = stack("minecraft:iron_helmet", vec![]);
        assert_eq!(
            ArmorKey::from_item(&iron, 0, &trims).unwrap().asset,
            "minecraft:iron"
        );
        assert!(ArmorKey::from_item(&iron, 3, &trims).is_none());
        let custom = stack(
            "minecraft:stone",
            vec![
                (
                    "minecraft:equippable",
                    Tag::Compound(vec![
                        ("slot".into(), string("chest")),
                        ("asset_id".into(), string("example:armor")),
                    ]),
                ),
                ("minecraft:dyed_color", Tag::Int(0x012345)),
            ],
        );
        let key = ArmorKey::from_item(&custom, 1, &trims).unwrap();
        assert_eq!(key.asset, "example:armor");
        assert_eq!(key.dye, Some(0x012345));
        assert!(ArmorKey::from_item(&custom, 0, &trims).is_none());
        let removed = stack(
            "minecraft:iron_helmet",
            vec![("!minecraft:equippable", Tag::Compound(vec![]))],
        );
        assert!(ArmorKey::from_item(&removed, 0, &trims).is_none());
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn vanilla_equipment_layers_trims_and_resource_pack_overrides_load() {
        use native_resources::packs::ResourceStack;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut provider = ResourceStack::open(&game).unwrap();
        let registry = TrimRegistry::load(&mut provider).unwrap();
        for material in [
            "leather",
            "golden",
            "copper",
            "chainmail",
            "iron",
            "diamond",
            "netherite",
        ] {
            for (slot, piece) in ["helmet", "chestplate", "leggings", "boots"]
                .into_iter()
                .enumerate()
            {
                let item = stack(&format!("minecraft:{material}_{piece}"), vec![]);
                let key = ArmorKey::from_item(&item, slot, &registry).unwrap();
                let appearance = load(&mut provider, &key).unwrap();
                assert_eq!(
                    appearance.layers.len(),
                    if material == "leather" { 2 } else { 1 }
                );
                assert_eq!(appearance.layers[0].texture.dimensions(), (64, 32));
            }
        }
        let turtle = stack("minecraft:turtle_helmet", vec![]);
        assert_eq!(
            load(
                &mut provider,
                &ArmorKey::from_item(&turtle, 0, &registry).unwrap()
            )
            .unwrap()
            .layers
            .len(),
            1
        );
        let red = stack(
            "minecraft:leather_chestplate",
            vec![("minecraft:dyed_color", Tag::Int(0xff0000))],
        );
        let leather = load(
            &mut provider,
            &ArmorKey::from_item(&red, 1, &registry).unwrap(),
        )
        .unwrap();
        assert_eq!(leather.layers[0].tint, [1.0, 0.0, 0.0]);
        assert_eq!(leather.layers[1].tint, [1.0; 3]);
        let trim = |material: &str| {
            Tag::Compound(vec![
                ("material".into(), string(material)),
                ("pattern".into(), string("minecraft:coast")),
            ])
        };
        let quartz = stack(
            "minecraft:iron_helmet",
            vec![("minecraft:trim", trim("minecraft:quartz"))],
        );
        let iron = stack(
            "minecraft:iron_helmet",
            vec![("minecraft:trim", trim("minecraft:iron"))],
        );
        let quartz = load(
            &mut provider,
            &ArmorKey::from_item(&quartz, 0, &registry).unwrap(),
        )
        .unwrap();
        let iron = load(
            &mut provider,
            &ArmorKey::from_item(&iron, 0, &registry).unwrap(),
        )
        .unwrap();
        assert_eq!(quartz.layers.len(), 2);
        assert_eq!(iron.layers.len(), 2);
        assert_ne!(
            quartz.layers[1].texture.as_raw(),
            iron.layers[1].texture.as_raw()
        );
        assert!(iron.layers[1].texture.pixels().any(|pixel| pixel[3] > 0));

        // The selected world data pack changes the same IDs' palette and pattern.
        let world = tempfile::tempdir().unwrap();
        let pack_root = world.path().join("datapacks/custom");
        std::fs::create_dir_all(pack_root.join("data/minecraft/trim_material")).unwrap();
        std::fs::create_dir_all(pack_root.join("data/minecraft/trim_pattern")).unwrap();
        std::fs::write(
            pack_root.join("pack.mcmeta"),
            br#"{"pack":{"description":"trim test","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        std::fs::write(
            pack_root.join("data/minecraft/trim_material/quartz.json"),
            br#"{"palette_id":"minecraft:trim/redstone","description":{"text":"Test"}}"#,
        )
        .unwrap();
        std::fs::write(
            pack_root.join("data/minecraft/trim_pattern/coast.json"),
            br#"{"asset_id":"minecraft:bolt","decal":false,"description":{"text":"Test"}}"#,
        )
        .unwrap();
        let mut world_provider = ResourceStack::open(&game).unwrap();
        let warnings = world_provider
            .configure_world_data(
                world.path(),
                &native_resources::datapacks::DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/custom".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )
            .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let world_registry = TrimRegistry::load(&mut world_provider).unwrap();
        let item = stack(
            "minecraft:iron_helmet",
            vec![("minecraft:trim", trim("minecraft:quartz"))],
        );
        let key = ArmorKey::from_item(&item, 0, &world_registry).unwrap();
        assert_eq!(key.trim.as_ref().unwrap().asset, "minecraft:bolt");
        assert_eq!(
            key.trim.as_ref().unwrap().palette,
            "minecraft:trim/redstone"
        );
        let changed = load(&mut world_provider, &key).unwrap();
        assert_ne!(
            changed.layers[1].texture.as_raw(),
            quartz.layers[1].texture.as_raw()
        );

        let pack = tempfile::tempdir().unwrap();
        let definition = pack.path().join("assets/minecraft/equipment/iron.json");
        std::fs::create_dir_all(definition.parent().unwrap()).unwrap();
        std::fs::write(
            &definition,
            br#"{"layers":{"humanoid":[{"texture":"example:custom"}]}}"#,
        )
        .unwrap();
        let image = pack
            .path()
            .join("assets/example/textures/entity/equipment/humanoid/custom.png");
        std::fs::create_dir_all(image.parent().unwrap()).unwrap();
        RgbaImage::from_pixel(64, 32, image::Rgba([11, 73, 137, 255]))
            .save(image)
            .unwrap();
        provider.push_pack(pack.path()).unwrap();
        let item = stack("minecraft:iron_helmet", vec![]);
        let custom = load(
            &mut provider,
            &ArmorKey::from_item(&item, 0, &registry).unwrap(),
        )
        .unwrap();
        assert_eq!(
            custom.layers[0].texture.get_pixel(5, 5).0,
            [11, 73, 137, 255]
        );
    }
}
