//! Resolve generation lighting settings from a world's selected data packs.
use crate::packs::{ResourceId, ResourceProvider};
use anyhow::{Context, Result, ensure};
use serde_json::Value;

/// Cache this together with the generation settings from the same pack revision.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedDimensionLighting {
    pub dimension: ResourceId,
    pub dimension_type: Option<ResourceId>,
    /// Encoded inline definition for sharing with render snapshots.
    pub inline_dimension_type: Option<std::sync::Arc<str>>,
    pub min_y: i32,
    pub height: i32,
    pub has_skylight: bool,
    /// Missing in incomplete synthetic definitions; gameplay checks that
    /// require ambient light must wait for a complete dimension type.
    pub ambient_light: Option<f32>,
}

impl GeneratedDimensionLighting {
    /// Resolve a dimension in the selected world preset. A named active
    /// dimension definition takes precedence over the preset's corresponding
    /// stem, and type definitions are read through the same data-pack stack.
    pub fn load_preset(
        provider: &mut impl ResourceProvider,
        preset: &str,
        dimension: &str,
    ) -> Result<Self> {
        Self::load_preset_with_generator(provider, preset, dimension).map(|(lighting, _)| lighting)
    }

    /// Resolve the generator and lighting from the same selected dimension stem.
    pub fn load_preset_with_generator(
        provider: &mut impl ResourceProvider,
        preset: &str,
        dimension: &str,
    ) -> Result<(Self, Value)> {
        let (lighting, generator, _) =
            Self::load_preset_with_definition(provider, preset, dimension)?;
        Ok((lighting, generator))
    }

    /// Resolve lighting, generator, and the exact named or inline type together.
    pub fn load_preset_with_definition(
        provider: &mut impl ResourceProvider,
        preset: &str,
        dimension: &str,
    ) -> Result<(Self, Value, Value)> {
        let preset = ResourceId::parse(preset)?;
        let dimension = ResourceId::parse(dimension)?;
        let path = format!(
            "data/{}/worldgen/world_preset/{}.json",
            preset.namespace, preset.path
        );
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing world preset {path}"))?;
        let preset: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid world preset {path}"))?;
        let dimensions = preset
            .get("dimensions")
            .and_then(Value::as_object)
            .context("preset dimensions are missing")?;
        let mut stem = None;
        for (key, value) in dimensions {
            if ResourceId::parse(key)? == dimension {
                ensure!(stem.is_none(), "duplicate dimension in world preset");
                stem = Some(value);
            }
        }
        let dimension_path = format!(
            "data/{}/dimension/{}.json",
            dimension.namespace, dimension.path
        );
        let override_stem = provider
            .read_data(&dimension_path)?
            .map(|bytes| {
                serde_json::from_slice::<Value>(&bytes)
                    .with_context(|| format!("invalid dimension {dimension_path}"))
            })
            .transpose()?;
        // WorldDimensions.bake takes the union of preset and data-pack
        // dimensions, preferring a registry entry when both contain a key.
        let stem = override_stem
            .as_ref()
            .or(stem)
            .context("dimension is missing from world preset and active data packs")?;
        let generator = stem
            .get("generator")
            .filter(|v| v.is_object())
            .context("dimension generator is missing or invalid")?
            .clone();
        let definition = stem.get("type").context("dimension type is missing")?;
        let (dimension_type, definition) = if let Some(name) = definition.as_str() {
            let id = ResourceId::parse(name)?;
            let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing dimension type {path}"))?;
            let value = serde_json::from_slice::<Value>(&bytes)
                .with_context(|| format!("invalid dimension type {path}"))?;
            (Some(id), value)
        } else {
            ensure!(
                definition.is_object(),
                "dimension type must be a reference or object"
            );
            (None, definition.clone())
        };
        let min_y = i32::try_from(
            definition
                .get("min_y")
                .and_then(Value::as_i64)
                .context("dimension min_y is missing or invalid")?,
        )?;
        let height = i32::try_from(
            definition
                .get("height")
                .and_then(Value::as_i64)
                .context("dimension height is missing or invalid")?,
        )?;
        ensure!(
            min_y % 16 == 0
                && height > 0
                && height % 16 == 0
                && min_y.checked_add(height).is_some(),
            "unsupported dimension height"
        );
        let has_skylight = definition
            .get("has_skylight")
            .and_then(Value::as_bool)
            .context("dimension has_skylight is missing or invalid")?;
        let ambient_light = definition
            .get("ambient_light")
            .map(|value| -> Result<f32> {
                let ambient = value
                    .as_f64()
                    .context("dimension ambient_light is invalid")?
                    as f32;
                ensure!(ambient.is_finite(), "dimension ambient_light is invalid");
                Ok(ambient)
            })
            .transpose()?;
        let inline_dimension_type = if dimension_type.is_none() {
            Some(std::sync::Arc::from(serde_json::to_string(&definition)?))
        } else {
            None
        };
        Ok((
            Self {
                dimension,
                dimension_type,
                inline_dimension_type,
                min_y,
                height,
                has_skylight,
                ambient_light,
            },
            generator,
            definition,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Packs(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Packs {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }
    fn packs() -> Packs {
        Packs(BTreeMap::from([
            ("data/example/worldgen/world_preset/test.json".into(),br#"{"dimensions":{"example:world":{"type":"example:base","generator":{"type":"minecraft:noise"}}}}"#.to_vec()),
            ("data/example/dimension_type/base.json".into(),br#"{"min_y":-64,"height":384,"has_skylight":true,"ambient_light":0.1}"#.to_vec()),
            ("data/example/dimension_type/override.json".into(),br#"{"min_y":0,"height":256,"has_skylight":false,"ambient_light":0.25}"#.to_vec()),
        ]))
    }
    #[test]
    fn preset_and_active_dimension_override_select_type_and_sky() {
        let mut p = packs();
        let original =
            GeneratedDimensionLighting::load_preset(&mut p, "example:test", "example:world")
                .unwrap();
        assert_eq!(
            (original.min_y, original.height, original.has_skylight),
            (-64, 384, true)
        );
        assert_eq!(original.ambient_light, Some(0.1));
        p.0.insert(
            "data/example/dimension/world.json".into(),
            br#"{"type":"example:override","generator":{"type":"example:override"}}"#.to_vec(),
        );
        let (selected, generator) = GeneratedDimensionLighting::load_preset_with_generator(
            &mut p,
            "example:test",
            "example:world",
        )
        .unwrap();
        assert_eq!(generator["type"], "example:override");
        assert_eq!(
            (selected.min_y, selected.height, selected.has_skylight),
            (0, 256, false)
        );
        assert_eq!(selected.ambient_light, Some(0.25));
        assert_eq!(
            selected.dimension_type.unwrap(),
            ResourceId::parse("example:override").unwrap()
        );
        p.0.insert("data/example/dimension/world.json".into(),br#"{"type":{"min_y":0,"height":16,"has_skylight":true,"ambient_light":0.5},"generator":{"type":"minecraft:noise"}}"#.to_vec());
        let inline =
            GeneratedDimensionLighting::load_preset(&mut p, "example:test", "example:world")
                .unwrap();
        assert_eq!(inline.dimension_type, None);
        assert_eq!(inline.ambient_light, Some(0.5));
        assert_eq!(
            (inline.min_y, inline.height, inline.has_skylight),
            (0, 16, true)
        );
    }
    #[test]
    fn active_data_pack_dimension_can_extend_the_selected_preset() {
        let mut p = packs();
        p.0.insert(
            "data/example/dimension/extra.json".into(),
            br#"{"type":"example:override","generator":{"type":"example:extra"}}"#.to_vec(),
        );
        let (lighting, generator) = GeneratedDimensionLighting::load_preset_with_generator(
            &mut p,
            "example:test",
            "example:extra",
        )
        .unwrap();
        assert_eq!(
            (lighting.min_y, lighting.height, lighting.has_skylight),
            (0, 256, false)
        );
        assert_eq!(generator["type"], "example:extra");
        p.0.insert(
            "data/example/dimension/extra.json".into(),
            b"invalid".to_vec(),
        );
        assert!(
            GeneratedDimensionLighting::load_preset(&mut p, "example:test", "example:extra")
                .is_err()
        );
    }
    #[test]
    fn rejects_unknown_or_invalid_lighting_settings() {
        let mut p = packs();
        assert!(
            GeneratedDimensionLighting::load_preset(&mut p, "example:test", "example:missing")
                .is_err()
        );
        for value in [
            br#"{"min_y":-64,"height":384}"#.as_slice(),
            br#"{"min_y":-63,"height":384,"has_skylight":true}"#,
            br#"{"min_y":0,"height":16,"has_skylight":1}"#,
            br#"{"min_y":0,"height":16,"has_skylight":true,"ambient_light":true}"#,
            br#"{"min_y":0,"height":16,"has_skylight":true,"ambient_light":1e100}"#,
        ] {
            p.0.insert(
                "data/example/dimension_type/base.json".into(),
                value.to_vec(),
            );
            assert!(
                GeneratedDimensionLighting::load_preset(&mut p, "example:test", "example:world")
                    .is_err()
            );
        }
    }
    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn installed_normal_preset_resolves_three_dimension_types() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut packs = crate::packs::ResourceStack::open(&game).unwrap();
        for (dimension, min_y, height, sky, ambient) in [
            ("minecraft:overworld", -64, 384, true, 0.0),
            ("minecraft:the_nether", 0, 256, false, 0.1),
            ("minecraft:the_end", 0, 256, true, 0.25),
        ] {
            let result =
                GeneratedDimensionLighting::load_preset(&mut packs, "minecraft:normal", dimension)
                    .unwrap();
            assert_eq!(
                (result.min_y, result.height, result.has_skylight),
                (min_y, height, sky)
            );
            assert_eq!(
                result.dimension_type,
                Some(ResourceId::parse(dimension).unwrap())
            );
            assert_eq!(result.ambient_light, Some(ambient));
        }
    }
}
