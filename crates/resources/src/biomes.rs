//! The color-related subset of 26.3 biome definitions.
//!
//! World data packs can replace these definitions. Callers must not mistake these
//! built-in colors for the world's fully resolved dynamic biome registry.

use std::{io::Cursor, sync::OnceLock};

use anyhow::{Context, Result, bail, ensure};
use image::{ImageFormat, RgbaImage};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};

const DEFAULT_GRASS: u32 = 0x91bd5a;
const DEFAULT_FOLIAGE: u32 = 0x7ab847;
const DEFAULT_DRY_FOLIAGE: u32 = 0xae9d77;

pub struct ColorMap(RgbaImage);

impl ColorMap {
    pub fn new(image: RgbaImage) -> Result<Self> {
        ensure!(
            image.dimensions() == (256, 256),
            "colormap must be 256 by 256 pixels"
        );
        Ok(Self(image))
    }

    /// Biome climate values are floats in Java; texture coordinates are doubles.
    pub fn lookup(&self, temperature: f32, downfall: f32) -> u32 {
        let temperature = f64::from(temperature.clamp(0.0, 1.0));
        let downfall = f64::from(downfall.clamp(0.0, 1.0)) * temperature;
        let x = ((1.0 - temperature) * 255.0) as u32;
        let y = ((1.0 - downfall) * 255.0) as u32;
        let [r, g, b, _] = self.0.get_pixel(x, y).0;
        (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
    }
}

#[derive(Default)]
pub struct ColorMaps {
    pub grass: Option<ColorMap>,
    pub foliage: Option<ColorMap>,
    pub dry_foliage: Option<ColorMap>,
}

impl ColorMaps {
    /// Color maps obey resource-pack precedence, including user replacements.
    pub fn load(source: &mut impl ResourceProvider, warnings: &mut Vec<String>) -> Self {
        fn read(
            source: &mut impl ResourceProvider,
            name: &str,
            warnings: &mut Vec<String>,
        ) -> Option<ColorMap> {
            let path = format!("assets/minecraft/textures/colormap/{name}.png");
            let result = (|| -> Result<ColorMap> {
                let bytes = source
                    .read_asset(&path)?
                    .with_context(|| format!("missing {path}"))?;
                let mut reader =
                    image::ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(256);
                limits.max_image_height = Some(256);
                limits.max_alloc = Some(4 * 1024 * 1024);
                reader.limits(limits);
                ColorMap::new(reader.decode()?.to_rgba8())
            })();
            match result {
                Ok(map) => Some(map),
                Err(error) => {
                    warnings.push(format!(
                        "cannot load {path}: {error:#}; using a fallback color"
                    ));
                    None
                }
            }
        }
        Self {
            grass: read(source, "grass", warnings),
            foliage: read(source, "foliage", warnings),
            dry_foliage: read(source, "dry_foliage", warnings),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrassModifier {
    None,
    DarkForest,
    Swamp,
}

#[derive(Clone, Debug)]
pub struct BiomeColors {
    grass: u32,
    pub foliage: u32,
    pub dry_foliage: u32,
    pub water: u32,
    pub grass_modifier: GrassModifier,
}

impl BiomeColors {
    pub fn load_world(
        source: &mut impl ResourceProvider,
        id: &str,
        maps: &ColorMaps,
    ) -> Result<Option<Self>> {
        let id = ResourceId::parse(id)?;
        let path = format!("data/{}/worldgen/biome/{}.json", id.namespace, id.path);
        source
            .read_data(&path)?
            .map(|bytes| Self::from_json(&bytes, maps))
            .transpose()
    }
    pub fn load_builtin(
        source: &mut impl ResourceProvider,
        id: &str,
        maps: &ColorMaps,
    ) -> Result<Option<Self>> {
        let id = ResourceId::parse(id)?;
        let path = format!("data/{}/worldgen/biome/{}.json", id.namespace, id.path);
        source
            .read_builtin_data(&path)?
            .map(|bytes| Self::from_json(&bytes, maps))
            .transpose()
    }

    pub fn from_json(bytes: &[u8], maps: &ColorMaps) -> Result<Self> {
        let value: Value = serde_json::from_slice(bytes).context("invalid biome JSON")?;
        let climate = |key: &str| -> Result<f32> {
            let value = value
                .get(key)
                .and_then(Value::as_f64)
                .with_context(|| format!("missing {key}"))? as f32;
            ensure!(value.is_finite(), "invalid biome {key}");
            Ok(value)
        };
        let temperature = climate("temperature")?;
        let downfall = climate("downfall")?;
        let effects = value
            .get("effects")
            .and_then(Value::as_object)
            .context("missing biome effects")?;
        let color = |key: &str| effects.get(key).map(parse_color).transpose();
        let map_color = |map: &Option<ColorMap>, fallback| {
            map.as_ref()
                .map_or(fallback, |map| map.lookup(temperature, downfall))
        };
        let modifier = effects
            .get("grass_color_modifier")
            .map(|value| {
                value
                    .as_str()
                    .context("grass_color_modifier must be a string")
            })
            .transpose()?
            .unwrap_or("none");
        let grass_modifier = match modifier {
            "none" => GrassModifier::None,
            "dark_forest" => GrassModifier::DarkForest,
            "swamp" => GrassModifier::Swamp,
            other => bail!("unsupported grass color modifier {other}"),
        };
        Ok(Self {
            grass: color("grass_color")?.unwrap_or_else(|| map_color(&maps.grass, DEFAULT_GRASS)),
            foliage: color("foliage_color")?
                .unwrap_or_else(|| map_color(&maps.foliage, DEFAULT_FOLIAGE)),
            dry_foliage: color("dry_foliage_color")?
                .unwrap_or_else(|| map_color(&maps.dry_foliage, DEFAULT_DRY_FOLIAGE)),
            water: color("water_color")?.context("missing biome water_color")?,
            grass_modifier,
        })
    }

    pub fn grass_at(&self, x: i32, z: i32) -> u32 {
        match self.grass_modifier {
            GrassModifier::None => self.grass,
            GrassModifier::DarkForest => ((self.grass & 0xfefefe) + 0x28340a) >> 1,
            GrassModifier::Swamp => {
                static NOISE: OnceLock<native_core::simplex::Simplex2D> = OnceLock::new();
                let noise = NOISE.get_or_init(|| native_core::simplex::Simplex2D::new(2345, true));
                if f64::from(noise.sample(f64::from(x) * 0.0225, f64::from(z) * 0.0225)) < -0.1 {
                    0x4c763c
                } else {
                    0x6a7039
                }
            }
        }
    }
}

fn parse_color(value: &Value) -> Result<u32> {
    if let Some(s) = value.as_str() {
        let hex = s.strip_prefix('#').context("color must begin with #")?;
        ensure!(
            hex.len() == 6 || hex.len() == 8,
            "color must have six or eight hexadecimal digits"
        );
        return Ok(u32::from_str_radix(hex, 16).context("invalid color")? & 0xffffff);
    }
    let number = value
        .as_i64()
        .context("color must be a hexadecimal string or integer")?;
    ensure!(
        (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&number),
        "color integer is out of range"
    );
    Ok(number as u32 & 0xffffff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn climate_colormap_uses_java_float_inputs_and_double_coordinates() {
        let mut image = RgbaImage::new(256, 256);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgba([x as u8, y as u8, 42, 255]);
        }
        let map = ColorMap::new(image).unwrap();
        assert_eq!(map.lookup(0.8, 0.4), 0x32ad2a);
        assert_eq!(map.lookup(2.0, 0.0), 0x00ff2a);
        assert_eq!(map.lookup(-0.2, 1.0), 0xffff2a);
    }

    #[test]
    fn overrides_and_forest_modifier_are_applied_and_malformed_colors_fail() {
        let data = br##"{"temperature":0.8,"downfall":0.4,"effects":{"water_color":"#3f76e4","grass_color":"#c0e080","foliage_color":-1,"grass_color_modifier":"dark_forest"}}"##;
        let biome = BiomeColors::from_json(data, &ColorMaps::default()).unwrap();
        assert_eq!(biome.water, 0x3f76e4);
        assert_eq!(biome.foliage, 0xffffff);
        assert_eq!(
            biome.grass_at(0, 0),
            ((0xc0e080 & 0xfefefe) + 0x28340a) >> 1
        );
        assert!(
            BiomeColors::from_json(
                br##"{"temperature":0.8,"downfall":0.4,"effects":{"water_color":"#oops"}}"##,
                &ColorMaps::default()
            )
            .is_err()
        );
    }
}
