//! Selection and positioning for Java 26.3 template features.
use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;

use crate::{
    packs::ResourceId,
    structure_pool::HolderValue,
    structure_transform::{Mirror, Rotation, transform_block},
};

#[derive(Clone, Debug)]
pub struct TemplateEntry {
    pub id: ResourceId,
    pub rotations: Vec<Rotation>,
    pub weight: i32,
}

#[derive(Clone, Debug)]
pub struct TemplateFeature {
    entries: Vec<TemplateEntry>,
    processors: Option<HolderValue>,
    total_weight: i32,
}

impl TemplateFeature {
    pub fn entries(&self) -> &[TemplateEntry] {
        &self.entries
    }

    pub fn processors(&self) -> Option<&HolderValue> {
        self.processors.as_ref()
    }

    pub fn decode(value: &Value) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("template feature type missing")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "template",
            "wrong template feature type"
        );
        let values = value
            .get("templates")
            .and_then(Value::as_array)
            .context("template feature entries missing")?;
        ensure!(values.len() <= 4096, "too many template feature entries");
        let mut entries = Vec::with_capacity(values.len());
        let mut total_weight = 0i32;
        for value in values {
            let weight = value
                .get("weight")
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .context("invalid template weight")?;
            ensure!(weight >= 0, "negative template weight");
            total_weight = total_weight
                .checked_add(weight)
                .context("template weight overflow")?;
            let data = value.get("data").context("template entry missing")?;
            let id = ResourceId::parse(
                data.get("id")
                    .and_then(Value::as_str)
                    .context("template identifier missing")?,
            )?;
            let rotations = match data.get("rotations") {
                None => vec![
                    Rotation::None,
                    Rotation::Clockwise90,
                    Rotation::Clockwise180,
                    Rotation::Counterclockwise90,
                ],
                Some(value) => value
                    .as_array()
                    .context("invalid template rotations")?
                    .iter()
                    .map(|rotation| match rotation.as_str() {
                        Some("none") => Ok(Rotation::None),
                        Some("clockwise_90") => Ok(Rotation::Clockwise90),
                        Some("180") => Ok(Rotation::Clockwise180),
                        Some("counterclockwise_90") => Ok(Rotation::Counterclockwise90),
                        _ => bail!("invalid template rotation"),
                    })
                    .collect::<Result<_>>()?,
            };
            entries.push(TemplateEntry {
                id,
                rotations,
                weight,
            });
        }
        let processors = match value.get("processors") {
            None => None,
            Some(Value::String(id)) => Some(HolderValue::Reference(ResourceId::parse(id)?)),
            Some(value @ Value::Object(_)) => Some(HolderValue::Inline(value.clone())),
            Some(_) => bail!("invalid template processors"),
        };
        Ok(Self {
            entries,
            processors,
            total_weight,
        })
    }

    /// Draw the weighted template, then its rotation, using the same RNG.
    pub fn select<'a>(
        &'a self,
        random: &mut DecorationRandom,
    ) -> Result<(&'a TemplateEntry, Rotation)> {
        let mut weight = random
            .next_int_bound(self.total_weight)
            .context("template list has no selectable entry")?;
        let entry = self
            .entries
            .iter()
            .find(|entry| {
                weight -= entry.weight;
                weight < 0
            })
            .context("template weight total mismatch")?;
        let count = i32::try_from(entry.rotations.len()).context("too many template rotations")?;
        let rotation = entry.rotations[random
            .next_int_bound(count)
            .context("template rotations are empty")?
            as usize];
        Ok((entry, rotation))
    }

    /// Java centers the rotated template horizontally, using integer division.
    pub fn origin(position: [i32; 3], size: [i32; 3], rotation: Rotation) -> [i32; 3] {
        let offset = transform_block(
            [-(size[0] / 2), 0, -(size[2] / 2)],
            Mirror::None,
            rotation,
            [0, 0, 0],
        );
        [
            position[0].wrapping_add(offset[0]),
            position[1],
            position[2].wrapping_add(offset[2]),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn selection_validates_configuration_and_handles_empty_choices() {
        let empty =
            TemplateFeature::decode(&json!({"type":"minecraft:template","templates":[]})).unwrap();
        assert!(empty.select(&mut DecorationRandom::new(0)).is_err());
        for invalid in [
            json!({"data":{"id":"minecraft:a"},"weight":-1}),
            json!({"data":{"id":"minecraft:a","rotations":["invalid"]},"weight":1}),
        ] {
            assert!(
                TemplateFeature::decode(
                    &json!({"type":"minecraft:template","templates":[invalid]})
                )
                .is_err()
            );
        }
        let empty_rotations = TemplateFeature::decode(&json!({"type":"minecraft:template","templates":[{"data":{"id":"minecraft:a","rotations":[]},"weight":1}]})).unwrap();
        assert!(
            empty_rotations
                .select(&mut DecorationRandom::new(0))
                .is_err()
        );
    }
}
