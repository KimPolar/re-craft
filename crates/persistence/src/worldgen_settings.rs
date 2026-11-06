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

fn dimension_stem_to_nbt(stem: &Value, nodes: &mut usize) -> Result<Tag> {
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
    json_to_nbt(stem, 0, nodes)
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
        entries.push((
            JavaString::from(id.as_str()),
            dimension_stem_to_nbt(stem, &mut nodes)?,
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

/// Capture the baked dimension registry used by a new world. Resource-backed
/// dimension stems override matching preset stems and add their own keys.
pub fn draft_baked_worldgen_settings(
    provider: &mut impl ResourceProvider,
    preset: &str,
    options: NewWorldOptions,
) -> Result<NamedTag> {
    let mut document = draft_worldgen_settings(provider, preset, options)?;
    let Tag::Compound(root) = &mut document.value else {
        unreachable!()
    };
    let Tag::Compound(data) = &mut root
        .iter_mut()
        .find(|(name, _)| name.matches("data"))
        .expect("draft data exists")
        .1
    else {
        unreachable!()
    };
    let Tag::Compound(dimensions) = &mut data
        .iter_mut()
        .find(|(name, _)| name.matches("dimensions"))
        .expect("draft dimensions exist")
        .1
    else {
        unreachable!()
    };
    let ids = provider.list_registry_data("dimension")?;
    ensure!(ids.len() <= 4096, "too many active dimensions");
    let mut nodes = 0;
    for id in ids {
        let path = format!("data/{}/dimension/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing active dimension {path}"))?;
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "dimension exceeds maximum size: {path}"
        );
        let stem: Value =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid dimension {path}"))?;
        let value = dimension_stem_to_nbt(&stem, &mut nodes)
            .with_context(|| format!("invalid dimension {path}"))?;
        let name = format!("{}:{}", id.namespace, id.path);
        if let Some((_, current)) = dimensions.iter_mut().find(|(key, _)| key.matches(&name)) {
            *current = value;
        } else {
            dimensions.push((JavaString::from(name.as_str()), value));
            ensure!(dimensions.len() <= 4096, "too many baked dimensions");
        }
    }
    Ok(document)
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
    fn baked_dimensions_include_active_overrides_and_extra_dimensions() {
        use std::collections::BTreeMap;
        struct Active {
            files: BTreeMap<String, Vec<u8>>,
            ids: Vec<ResourceId>,
        }
        impl ResourceProvider for Active {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(self.files.get(path).cloned())
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                ensure!(folder == "dimension", "unexpected registry");
                Ok(self.ids.clone())
            }
        }
        let mut packs = Active {
            files: BTreeMap::new(),
            ids: vec![
                ResourceId::parse("minecraft:overworld").unwrap(),
                ResourceId::parse("example:sky").unwrap(),
            ],
        };
        packs.files.insert(
            "data/example/worldgen/world_preset/custom.json".into(),
            serde_json::to_vec(
                &json!({"dimensions":{"minecraft:overworld":stem(),"minecraft:the_nether":stem()}}),
            )
            .unwrap(),
        );
        let override_stem =
            json!({"type":"example:overworld_type","generator":{"type":"example:override"}});
        let extra_stem = json!({"type":"example:sky_type","generator":{"type":"example:sky"}});
        packs.files.insert(
            "data/minecraft/dimension/overworld.json".into(),
            serde_json::to_vec(&override_stem).unwrap(),
        );
        packs.files.insert(
            "data/example/dimension/sky.json".into(),
            serde_json::to_vec(&extra_stem).unwrap(),
        );
        let draft = draft_baked_worldgen_settings(&mut packs, "example:custom", options()).unwrap();
        let dimensions = draft.value.get("data").unwrap().get("dimensions").unwrap();
        assert_eq!(
            dimensions
                .get("minecraft:overworld")
                .unwrap()
                .get("generator")
                .unwrap()
                .get("type"),
            Some(&Tag::String("example:override".into()))
        );
        assert_eq!(
            dimensions
                .get("example:sky")
                .unwrap()
                .get("generator")
                .unwrap()
                .get("type"),
            Some(&Tag::String("example:sky".into()))
        );
        assert!(dimensions.get("minecraft:the_nether").is_some());
        assert_eq!(
            crate::nbt::read(&crate::nbt::write(&draft).unwrap()).unwrap(),
            draft
        );
        packs.files.remove("data/example/dimension/sky.json");
        assert!(draft_baked_worldgen_settings(&mut packs, "example:custom", options()).is_err());
        packs.files.insert(
            "data/example/dimension/sky.json".into(),
            br#"{"type":"example:sky_type","generator":{}}"#.to_vec(),
        );
        assert!(draft_baked_worldgen_settings(&mut packs, "example:custom", options()).is_err());
        packs.ids.push(ResourceId::parse("example:sky").unwrap());
        assert!(draft_baked_worldgen_settings(&mut packs, "example:custom", options()).is_err());
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
