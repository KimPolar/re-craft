//! Reconstruct Java atlas layouts without extracting or distributing assets.
//!
//! Unknown source types or unsupported resources are reported to the caller, so
//! an approximation can be used instead of silently returning misleading UVs.

use std::{collections::BTreeMap, io::Cursor};

use anyhow::{Context, Result, bail, ensure};
use image::{ImageFormat, ImageReader};
use regex::Regex;
use serde_json::Value;

use crate::{
    packs::{ResourceId, ResourceProvider, ResourceStack},
    sprites::frame_dimensions,
    vanilla_stitcher::{SpriteSize, StitchLayout, effective_mip_level, stitch},
};

const MAX_TEXTURE_SIDE: u32 = 4096;
const MAX_ATLAS_SPRITES: usize = 100_000;

#[derive(Clone)]
enum Source {
    Image(ResourceId),
    Palette {
        image: ResourceId,
        key: ResourceId,
        palette: ResourceId,
    },
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("missing atlas source {name}"))
}

fn id(value: &str) -> Result<ResourceId> {
    ResourceId::parse(value)
}

fn matching(pattern: Option<&Value>) -> Result<Option<Regex>> {
    pattern
        .map(|p| -> Result<Regex> {
            let p = p
                .as_str()
                .context("atlas filter pattern must be a string")?;
            Regex::new(&format!("\\A(?:{p})\\z")).context("unsupported atlas filter pattern")
        })
        .transpose()
}

/// Reconstruct the item atlas used by Java 26.3 with texture filtering set to
/// `None`. Its item atlas has no mipmaps and uses a one-pixel sprite gutter.
/// The caller should cache the result per resource-pack stack and compute it off
/// the UI thread. The size limit matches the usual desktop GPU limit.
pub fn item_layout(stack: &mut ResourceStack) -> Result<StitchLayout> {
    atlas_layout(stack, "items", 0)
}

/// Reconstruct the block atlas with Java's effective mipmap level.
pub fn block_layout(stack: &mut ResourceStack, requested_mip: u32) -> Result<StitchLayout> {
    atlas_layout(stack, "blocks", requested_mip)
}

fn atlas_layout(
    stack: &mut ResourceStack,
    atlas: &str,
    requested_mip: u32,
) -> Result<StitchLayout> {
    let atlas_documents =
        stack.read_asset_stack(&format!("assets/minecraft/atlases/{atlas}.json"))?;
    ensure!(
        !atlas_documents.is_empty(),
        "missing {atlas} atlas definition"
    );
    let mut sources = BTreeMap::<ResourceId, Source>::new();
    for bytes in atlas_documents {
        let document: Value = serde_json::from_slice(&bytes).context("invalid item atlas JSON")?;
        let entries = document
            .get("sources")
            .and_then(Value::as_array)
            .context("missing atlas sources")?;
        for entry in entries {
            let kind = string(entry, "type")?
                .strip_prefix("minecraft:")
                .unwrap_or(string(entry, "type")?);
            match kind {
                "directory" => {
                    let folder = string(entry, "source")?;
                    let prefix = string(entry, "prefix")?;
                    for texture in stack.list_textures(folder)? {
                        let relative = texture
                            .path
                            .strip_prefix(folder)
                            .and_then(|p| p.strip_prefix('/'))
                            .context("atlas texture is outside its source directory")?;
                        let sprite = id(&format!("{}:{prefix}{relative}", texture.namespace))?;
                        sources.insert(sprite, Source::Image(texture));
                    }
                }
                "single" => {
                    let resource = id(string(entry, "resource")?)?;
                    let sprite = match entry.get("sprite") {
                        Some(value) => id(value.as_str().context("invalid sprite id")?)?,
                        None => resource.clone(),
                    };
                    if stack
                        .read_asset(&resource.asset_path("textures", "png"))?
                        .is_some()
                    {
                        sources.insert(sprite, Source::Image(resource));
                    }
                }
                "paletted_permutations" => {
                    let key = id(string(entry, "palette_key")?)?;
                    let textures = entry
                        .get("textures")
                        .and_then(Value::as_array)
                        .context("missing palette textures")?;
                    let permutations = entry
                        .get("permutations")
                        .and_then(Value::as_object)
                        .context("missing palette permutations")?;
                    for texture in textures {
                        let texture = id(texture.as_str().context("invalid palette texture")?)?;
                        if stack
                            .read_asset(&texture.asset_path("textures", "png"))?
                            .is_none()
                        {
                            continue;
                        }
                        for (suffix, palette) in permutations {
                            let sprite =
                                id(&format!("{}:{}_{suffix}", texture.namespace, texture.path))?;
                            let palette = id(palette.as_str().context("invalid palette id")?)?;
                            sources.insert(
                                sprite,
                                Source::Palette {
                                    image: texture.clone(),
                                    key: key.clone(),
                                    palette,
                                },
                            );
                        }
                    }
                }
                "filter" => {
                    let pattern = entry
                        .get("pattern")
                        .and_then(Value::as_object)
                        .context("missing atlas filter pattern")?;
                    let namespace = matching(pattern.get("namespace"))?;
                    let path = matching(pattern.get("path"))?;
                    sources.retain(|id, _| {
                        !(namespace.as_ref().is_none_or(|p| p.is_match(&id.namespace))
                            && path.as_ref().is_none_or(|p| p.is_match(&id.path)))
                    });
                }
                _ => bail!("unsupported item atlas source {kind}"),
            }
            ensure!(
                sources.len() <= MAX_ATLAS_SPRITES,
                "too many item atlas sprites"
            );
        }
    }

    let mut dimensions = BTreeMap::<ResourceId, (u32, u32)>::new();
    let mut palettes = BTreeMap::<ResourceId, u64>::new();
    let mut sprites = Vec::with_capacity(sources.len() + 1);
    for (id, source) in sources {
        let texture = match source {
            Source::Image(texture) => texture,
            Source::Palette {
                image,
                key,
                palette,
            } => {
                let key_size = palette_size(stack, &mut palettes, &key)?;
                let palette_size = palette_size(stack, &mut palettes, &palette)?;
                ensure!(
                    key_size == palette_size,
                    "atlas palettes have different sizes"
                );
                image
            }
        };
        let (width, height) = if let Some(size) = dimensions.get(&texture) {
            *size
        } else {
            let path = texture.asset_path("textures", "png");
            let asset = stack
                .read_asset_with_metadata(&path)?
                .with_context(|| format!("missing atlas texture {path}"))?;
            let (width, height) = png_dimensions(asset.bytes)
                .with_context(|| format!("invalid atlas texture {path}"))?;
            let size = frame_dimensions(width, height, asset.metadata.as_deref())
                .with_context(|| format!("invalid atlas animation {path}"))?;
            dimensions.insert(texture, size);
            size
        };
        if id != ResourceId::parse("missingno")? {
            sprites.push(SpriteSize { id, width, height });
        }
    }
    sprites.push(SpriteSize {
        id: ResourceId::parse("missingno")?,
        width: 16,
        height: 16,
    });
    let mip = effective_mip_level(&sprites, requested_mip)?;
    stitch(&sprites, 16_384, 16_384, mip, 0)
}

fn palette_size(
    stack: &mut ResourceStack,
    cache: &mut BTreeMap<ResourceId, u64>,
    id: &ResourceId,
) -> Result<u64> {
    if let Some(size) = cache.get(id) {
        return Ok(*size);
    }
    let path = id.asset_path("textures/palettes", "png");
    let bytes = stack
        .read_asset(&path)?
        .with_context(|| format!("missing atlas palette {path}"))?;
    let (width, height) =
        png_dimensions(bytes).with_context(|| format!("invalid atlas palette {path}"))?;
    let size = u64::from(width) * u64::from(height);
    cache.insert(id.clone(), size);
    Ok(size)
}

fn png_dimensions(bytes: Vec<u8>) -> Result<(u32, u32)> {
    // Decode completely: Java skips corrupt PNGs rather than stitching their headers.
    // On an unsupported image, return an error so callers use the fallback UVs.
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_TEXTURE_SIDE);
    limits.max_image_height = Some(MAX_TEXTURE_SIDE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?;
    ensure!(image.width() > 0 && image.height() > 0, "empty PNG");
    Ok((image.width(), image.height()))
}
