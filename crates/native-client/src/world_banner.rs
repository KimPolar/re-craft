//! Saved banner appearance and resource-pack aware flag textures.
use anyhow::{Context, Result, ensure};
use glam::IVec3;
use image::{ImageFormat, ImageReader, RgbaImage};
use native_persistence::nbt::{self, Tag, TagType};
use native_resources::packs::{ResourceId, ResourceProvider};
use native_world::loaded::{ChunkPos, SavedBlockEntities, World};
use std::{collections::BTreeMap, io::Cursor};

use crate::banner_item::{self, Layer};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Appearance {
    pub color: u32,
    pub layers: Vec<Layer>,
}

pub fn scan(world: &World, chunk: ChunkPos) -> Result<BTreeMap<[i32; 3], Appearance>> {
    let Some(SavedBlockEntities::Encoded(bytes)) = world.visible_block_entities(&chunk) else {
        return Ok(BTreeMap::new());
    };
    let document = nbt::read(bytes)?;
    let Tag::List {
        element_type: TagType::Compound,
        values,
    } = document.value
    else {
        anyhow::bail!("invalid block entity list")
    };
    let mut banners = BTreeMap::new();
    for entity in values {
        if !matches!(entity.get("id"),Some(Tag::String(id)) if id.matches("minecraft:banner") || id.matches("banner"))
        {
            continue;
        }
        let coord = |key| match entity.get(key) {
            Some(Tag::Int(v)) => Some(*v),
            _ => None,
        };
        let Some(position) = coord("x")
            .zip(coord("y"))
            .zip(coord("z"))
            .map(|((x, y), z)| IVec3::new(x, y, z))
        else {
            continue;
        };
        if position.x.div_euclid(16) != chunk.x || position.z.div_euclid(16) != chunk.z {
            continue;
        }
        let Some(state) = world.block(position.x, position.y, position.z) else {
            continue;
        };
        let Some(id) = native_world::collision::banner_item_id(state) else {
            continue;
        };
        let Some(color) = id
            .strip_prefix("minecraft:")
            .and_then(|s| s.strip_suffix("_banner"))
            .and_then(banner_item::color)
        else {
            continue;
        };
        let components = Tag::Compound(
            entity
                .get("patterns")
                .map(|v| vec![("minecraft:banner_patterns".into(), v.clone())])
                .unwrap_or_default(),
        );
        let Some(layers) = banner_item::patterns(&components) else {
            continue;
        };
        banners.insert(position.to_array(), Appearance { color, layers });
    }
    Ok(banners)
}

fn texture(provider: &mut impl ResourceProvider, id: &ResourceId) -> Result<RgbaImage> {
    let path = id.asset_path("textures", "png");
    let asset = provider
        .read_asset_with_metadata(&path)?
        .with_context(|| format!("missing banner texture {path}"))?;
    let mut reader = ImageReader::with_format(Cursor::new(asset.bytes), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?.to_rgba8();
    native_resources::sprites::first_frame(&image, asset.metadata.as_deref())
}

pub fn compose(provider: &mut impl ResourceProvider, appearance: &Appearance) -> Result<RgbaImage> {
    let mut flag = texture(
        provider,
        &ResourceId::parse("minecraft:entity/banner/banner_base")?,
    )?;
    let base = texture(
        provider,
        &ResourceId::parse("minecraft:entity/banner/base")?,
    )?;
    banner_item::overlay(&mut flag, &base, appearance.color)?;
    ensure!(appearance.layers.len() <= 16, "too many banner layers");
    for layer in &appearance.layers {
        let asset = banner_item::asset_id(provider, &layer.pattern)?;
        let id = ResourceId {
            namespace: asset.namespace,
            path: format!("entity/banner/{}", asset.path),
        };
        banner_item::overlay(&mut flag, &texture(provider, &id)?, layer.color)?;
    }
    Ok(flag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba};

    struct Provider;
    impl ResourceProvider for Provider {
        fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            let pixel = match path {
                "assets/minecraft/textures/entity/banner/banner_base.png" => {
                    Rgba([255, 255, 255, 255])
                }
                "assets/minecraft/textures/entity/banner/base.png" => Rgba([255, 255, 255, 0]),
                "assets/example/textures/entity/banner/design.png" => Rgba([255, 255, 255, 255]),
                _ => return Ok(None),
            };
            let mut bytes = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, pixel))
                .write_to(&mut bytes, ImageFormat::Png)?;
            Ok(Some(bytes.into_inner()))
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(
                (path == "data/example/banner_pattern/custom.json").then(|| {
                    br#"{"asset_id":"example:design","translation_key":"example.banner"}"#.to_vec()
                }),
            )
        }
    }

    #[test]
    fn composition_resolves_data_pack_patterns_and_resource_textures() {
        let appearance = Appearance {
            color: 0xff0000,
            layers: vec![Layer {
                pattern: banner_item::Pattern::Registry("example:custom".into()),
                color: 0x123456,
            }],
        };
        let image = compose(&mut Provider, &appearance).unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [0x12, 0x34, 0x56, 255]);
    }
}
