use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use native_core::random::LegacyRandom;
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelChoice {
    pub model: ResourceId,
    pub x_rotation: u16,
    pub y_rotation: u16,
    pub uv_lock: bool,
    pub weight: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockstateDefinition {
    pub multipart: bool,
    pub groups: Vec<Vec<ModelChoice>>,
}

/// Each returned group is a weighted choice; multipart states may have several groups.
pub fn resolve_blockstate(
    source: &mut impl ResourceProvider,
    id: &str,
    properties: &BTreeMap<String, String>,
) -> Result<Vec<Vec<ModelChoice>>> {
    Ok(resolve_definition(source, id, properties)?.groups)
}

pub fn resolve_definition(
    source: &mut impl ResourceProvider,
    id: &str,
    properties: &BTreeMap<String, String>,
) -> Result<BlockstateDefinition> {
    let id = ResourceId::parse(id)?;
    let path = id.asset_path("blockstates", "json");
    let bytes = source
        .read_asset(&path)?
        .with_context(|| format!("missing blockstate {path}"))?;
    let value: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid blockstate JSON {path}"))?;
    let root = value.as_object().context("blockstate must be an object")?;
    ensure!(
        !(root.contains_key("variants") && root.contains_key("multipart")),
        "blockstate has both variants and multipart"
    );
    if let Some(variants) = root.get("variants") {
        let variants = variants
            .as_object()
            .context("blockstate variants must be an object")?;
        let mut matched = Vec::new();
        for (key, value) in variants {
            if matches_variant(key, properties)? {
                matched.push(choices(value)?);
            }
        }
        ensure!(
            matched.len() == 1,
            "blockstate has {} matching variants",
            matched.len()
        );
        Ok(BlockstateDefinition {
            multipart: false,
            groups: matched,
        })
    } else if let Some(multipart) = root.get("multipart") {
        let parts = multipart
            .as_array()
            .context("blockstate multipart must be an array")?;
        let mut matched = Vec::new();
        for part in parts {
            let part = part
                .as_object()
                .context("multipart entry must be an object")?;
            if part
                .get("when")
                .map(|when| matches_condition(when, properties))
                .transpose()?
                .unwrap_or(true)
            {
                matched.push(choices(
                    part.get("apply")
                        .context("multipart entry is missing apply")?,
                )?);
            }
        }
        Ok(BlockstateDefinition {
            multipart: true,
            groups: matched,
        })
    } else {
        bail!("blockstate has neither variants nor multipart")
    }
}

/// Default block position seed. Some block types override this and require their own seed.
pub fn positional_seed(x: i32, y: i32, z: i32) -> i64 {
    let value = i64::from(x.wrapping_mul(3_129_871))
        ^ i64::from(z).wrapping_mul(116_129_781)
        ^ i64::from(y);
    value
        .wrapping_mul(value)
        .wrapping_mul(42_317_861)
        .wrapping_add(value.wrapping_mul(11))
        >> 16
}

impl BlockstateDefinition {
    /// Returns the selected index in each group using the same random order as 26.3.
    /// `seed` must come from the block's own seed rule, which can differ from the default.
    pub fn select_indices(&self, seed: i64) -> Result<Vec<usize>> {
        let mut random = LegacyRandom::new(seed);
        let shared_seed = self.multipart.then(|| random.next_long());
        self.groups
            .iter()
            .map(|group| {
                if let Some(seed) = shared_seed {
                    random = LegacyRandom::new(seed);
                }
                let total = group.iter().try_fold(0i32, |total, choice| {
                    let weight = i32::try_from(choice.weight)
                        .context("model weight exceeds 32-bit range")?;
                    ensure!(weight > 0, "model weight must be positive");
                    total
                        .checked_add(weight)
                        .context("total model weight exceeds 32-bit range")
                })?;
                let mut selected = random
                    .next_int_bound(total)
                    .context("weighted model group is empty")?;
                for (index, choice) in group.iter().enumerate() {
                    selected -= choice.weight as i32;
                    if selected < 0 {
                        return Ok(index);
                    }
                }
                bail!("weighted model group has no selection")
            })
            .collect()
    }
}

fn matches_variant(key: &str, properties: &BTreeMap<String, String>) -> Result<bool> {
    if key.is_empty() {
        return Ok(true);
    }
    for item in key.split(',') {
        let (name, expected) = item
            .split_once('=')
            .with_context(|| format!("invalid blockstate variant {key}"))?;
        ensure!(
            !name.is_empty() && !expected.is_empty(),
            "invalid blockstate variant {key}"
        );
        if !properties
            .get(name)
            .is_some_and(|actual| expected.split('|').any(|candidate| candidate == actual))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn matches_condition(value: &Value, properties: &BTreeMap<String, String>) -> Result<bool> {
    let object = value
        .as_object()
        .context("multipart condition must be an object")?;
    for (name, expected) in object {
        if name == "OR" || name == "AND" {
            let operands = expected
                .as_array()
                .context("multipart logical condition must be an array")?;
            ensure!(
                !operands.is_empty(),
                "multipart logical condition cannot be empty"
            );
            let values = operands
                .iter()
                .map(|item| matches_condition(item, properties))
                .collect::<Result<Vec<_>>>()?;
            if name == "OR" && !values.iter().any(|value| *value) {
                return Ok(false);
            }
            if name == "AND" && !values.iter().all(|value| *value) {
                return Ok(false);
            }
        } else {
            let expected = expected
                .as_str()
                .context("multipart property condition must be a string")?;
            let actual = properties
                .get(name)
                .with_context(|| format!("unresolved block-state property {name}"))?;
            if !expected.split('|').any(|candidate| candidate == actual) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn choices(value: &Value) -> Result<Vec<ModelChoice>> {
    let values: Vec<&Value> = match value {
        Value::Object(_) => vec![value],
        Value::Array(values) if !values.is_empty() => values.iter().collect(),
        _ => bail!("blockstate model must be an object or nonempty array"),
    };
    values
        .into_iter()
        .map(|value| {
            let object = value
                .as_object()
                .context("blockstate model must be an object")?;
            let model = ResourceId::parse(
                object
                    .get("model")
                    .and_then(Value::as_str)
                    .context("blockstate model is missing model identifier")?,
            )?;
            let rotation = |field| -> Result<u16> {
                let value = object
                    .get(field)
                    .map(|value| {
                        value
                            .as_u64()
                            .context("model rotation must be a nonnegative integer")
                    })
                    .transpose()?
                    .unwrap_or(0);
                ensure!(
                    matches!(value, 0 | 90 | 180 | 270),
                    "model rotation must be 0, 90, 180 or 270"
                );
                Ok(value as u16)
            };
            let uv_lock = object
                .get("uvlock")
                .map(|value| value.as_bool().context("uvlock must be a boolean"))
                .transpose()?
                .unwrap_or(false);
            let weight = object
                .get("weight")
                .map(|value| {
                    value
                        .as_u64()
                        .context("model weight must be a positive integer")
                })
                .transpose()?
                .unwrap_or(1);
            ensure!(
                (1..=i32::MAX as u64).contains(&weight),
                "model weight must be a positive 32-bit integer"
            );
            Ok(ModelChoice {
                model,
                x_rotation: rotation("x")?,
                y_rotation: rotation("y")?,
                uv_lock,
                weight: weight as u32,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemorySource(&'static str);
    impl ResourceProvider for MemorySource {
        fn read_asset(&mut self, _path: &str) -> Result<Option<Vec<u8>>> {
            Ok(Some(self.0.as_bytes().to_vec()))
        }
    }

    #[test]
    fn variants_respect_state_and_keep_weighted_choices() {
        let mut source = MemorySource(
            r#"{"variants":{"snowy=false":[{"model":"block/a","weight":2},{"model":"block/b","y":90}],"snowy=true":{"model":"block/c"}}}"#,
        );
        let props = BTreeMap::from([("snowy".to_owned(), "false".to_owned())]);
        let groups = resolve_blockstate(&mut source, "grass_block", &props).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0][0].weight, 2);
        assert_eq!(groups[0][1].y_rotation, 90);
    }

    #[test]
    fn multipart_supports_or_and_multiple_model_groups() {
        let mut source = MemorySource(
            r#"{"multipart":[{"apply":{"model":"block/post"}},{"when":{"OR":[{"north":"true"},{"south":"true"}]},"apply":{"model":"block/side"}}]}"#,
        );
        let props = BTreeMap::from([
            ("north".to_owned(), "false".to_owned()),
            ("south".to_owned(), "true".to_owned()),
        ]);
        assert_eq!(
            resolve_blockstate(&mut source, "fence", &props)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn multipart_requires_properties_for_conditional_parts() {
        let mut source = MemorySource(
            r#"{"multipart":[{"apply":{"model":"block/post"}},{"when":{"north":"true"},"apply":{"model":"block/side"}}]}"#,
        );
        let error = resolve_blockstate(&mut source, "fence", &BTreeMap::new()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unresolved block-state property north")
        );
    }
}
