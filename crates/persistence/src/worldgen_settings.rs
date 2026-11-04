//! Draft the 26.3 world-generation saved-data document from a selected preset.
//!
//! This prepares NBT only. Generator definitions and registry references must
//! still be validated against the same active data packs before any world is
//! created or saved.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use native_resources::packs::{ResourceId, ResourceProvider};
use serde_json::Value;

use crate::{
    nbt::{JavaString, NamedTag, Tag, TagType},
    world::TARGET_DATA_VERSION,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NewWorldOptions {
    pub seed: i64,
    pub generate_structures: bool,
    pub bonus_chest: bool,
}

fn json_to_nbt(value: &Value, depth: usize, nodes: &mut usize) -> Result<Tag> {
    ensure!(depth <= 128, "world preset exceeds maximum nesting depth");
    *nodes = nodes
        .checked_add(1)
        .context("world preset node count overflow")?;
    ensure!(
        *nodes <= 1_000_000,
        "world preset exceeds maximum node count"
    );
    Ok(match value {
        Value::Null => bail!("world preset contains null"),
        Value::Bool(value) => Tag::Byte(i8::from(*value)),
        Value::String(value) => Tag::String(JavaString::from(value.as_str())),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                i32::try_from(value).map_or(Tag::Long(value), Tag::Int)
            } else if value.is_u64() {
                bail!("world preset integer exceeds signed 64-bit range")
            } else {
                let value = value.as_f64().context("invalid world preset number")?;
                ensure!(value.is_finite(), "non-finite world preset number");
                Tag::Double(value)
            }
        }
        Value::Array(values) => {
            let values = values
                .iter()
                .map(|value| json_to_nbt(value, depth + 1, nodes))
                .collect::<Result<Vec<_>>>()?;
            let element_type = values.first().map_or(TagType::End, Tag::tag_type);
            ensure!(
                values.iter().all(|value| value.tag_type() == element_type),
                "world preset contains a heterogeneous list"
            );
            Tag::List {
                element_type,
                values,
            }
        }
        Value::Object(values) => Tag::Compound(
            values
                .iter()
                .map(|(key, value)| {
                    Ok((
                        JavaString::from(key.as_str()),
                        json_to_nbt(value, depth + 1, nodes)?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?,
        ),
    })
}

/// Read the selected preset from the active pack stack and encode the
/// `data/minecraft/world_gen_settings.dat` document in memory. Every dimension
/// stem is retained, including custom dimensions; no files are written.
pub fn draft_worldgen_settings(
    provider: &mut impl ResourceProvider,
    preset: &str,
    options: NewWorldOptions,
) -> Result<NamedTag> {
    let preset = ResourceId::parse(preset)?;
    let path = format!(
        "data/{}/worldgen/world_preset/{}.json",
        preset.namespace, preset.path
    );
    let bytes = provider
        .read_data(&path)?
        .with_context(|| format!("missing world preset {path}"))?;
    ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "world preset exceeds maximum size"
    );
    let preset: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("invalid world preset {path}"))?;
    let dimensions = preset
        .get("dimensions")
        .and_then(Value::as_object)
        .context("world preset dimensions are missing")?;
    ensure!(
        dimensions.len() <= 4096,
        "world preset has too many dimensions"
    );
    let mut seen = BTreeSet::new();
    let mut entries = Vec::with_capacity(dimensions.len());
    let mut nodes = 0;
    for (name, stem) in dimensions {
        let id = ResourceId::parse(name)?;
        let id = format!("{}:{}", id.namespace, id.path);
        ensure!(seen.insert(id.clone()), "duplicate dimension {id}");
        let stem_object = stem
            .as_object()
            .context("dimension stem must be a compound")?;
        let dimension_type = stem_object
            .get("type")
            .context("dimension type is missing")?;
        if let Some(name) = dimension_type.as_str() {
            ResourceId::parse(name)?;
        } else {
            ensure!(
                dimension_type.is_object(),
                "dimension type must be a reference or compound"
            );
        }
        let generator = stem_object
            .get("generator")
            .and_then(Value::as_object)
            .context("dimension generator is missing")?;
        ResourceId::parse(
            generator
                .get("type")
                .and_then(Value::as_str)
                .context("generator type is missing")?,
        )?;
        entries.push((
            JavaString::from(id.as_str()),
            json_to_nbt(stem, 0, &mut nodes)?,
        ));
    }
    ensure!(
        seen.contains("minecraft:overworld"),
        "world preset has no overworld"
    );
    Ok(NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
            (
                "data".into(),
                Tag::Compound(vec![
                    ("seed".into(), Tag::Long(options.seed)),
                    (
                        "generate_structures".into(),
                        Tag::Byte(i8::from(options.generate_structures)),
                    ),
                    (
                        "bonus_chest".into(),
                        Tag::Byte(i8::from(options.bonus_chest)),
                    ),
                    ("dimensions".into(), Tag::Compound(entries)),
                ]),
            ),
        ]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Packs(Value);
    impl ResourceProvider for Packs {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok((path == "data/example/worldgen/world_preset/custom.json")
                .then(|| self.0.to_string().into_bytes()))
        }
    }
    fn options() -> NewWorldOptions {
        NewWorldOptions {
            seed: -42,
            generate_structures: false,
            bonus_chest: true,
        }
    }
    fn stem() -> Value {
        json!({"type":"minecraft:overworld","generator":{"type":"minecraft:noise","settings":"example:noise","biome_source":{"type":"minecraft:fixed","biome":"example:biome"}}})
    }

    #[test]
    fn keeps_custom_dimension_stems_and_world_options() {
        let mut packs = Packs(
            json!({"dimensions":{"overworld":stem(),"example:other":{"type":{"min_y":-64,"height":384,"has_skylight":true},"generator":{"type":"example:custom","values":[1,2]}}}}),
        );
        let document = draft_worldgen_settings(&mut packs, "example:custom", options()).unwrap();
        let data = document.value.get("data").unwrap();
        assert_eq!(data.get("seed"), Some(&Tag::Long(-42)));
        assert_eq!(data.get("generate_structures"), Some(&Tag::Byte(0)));
        assert_eq!(data.get("bonus_chest"), Some(&Tag::Byte(1)));
        let dims = data.get("dimensions").unwrap();
        assert!(dims.get("minecraft:overworld").is_some());
        assert!(dims.get("example:other").is_some());
        let bytes = crate::nbt::write(&document).unwrap();
        assert_eq!(crate::nbt::read(&bytes).unwrap(), document);
    }

    #[test]
    fn rejects_incomplete_or_unrepresentable_stems() {
        for value in [
            json!({"dimensions":{"example:other":stem()}}),
            json!({"dimensions":{"overworld":stem(),"minecraft:overworld":stem()}}),
            json!({"dimensions":{"minecraft:overworld":{"type":"minecraft:overworld","generator":{}}}}),
            json!({"dimensions":{"minecraft:overworld":{"type":"minecraft:overworld","generator":{"type":"example:custom","unknown":null}}}}),
            json!({"dimensions":{"minecraft:overworld":{"type":"minecraft:overworld","generator":{"type":"example:custom","unknown":[1,"two"]}}}}),
        ] {
            assert!(
                draft_worldgen_settings(&mut Packs(value), "example:custom", options()).is_err()
            );
        }
    }
}
