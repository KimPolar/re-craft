//! Initial ruined-portal choices from Java 26.3.
//!
//! These choices alone do not establish a structure start or place any blocks.
use anyhow::{Context, Result, ensure};
use native_core::{beardifier::BoundingBox, random::LegacyRandom};
use serde_json::Value;

use crate::{
    packs::ResourceId,
    structure_start::Heightmap,
    structure_transform::{Mirror, Rotation},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerticalPlacement {
    OnLandSurface,
    PartlyBuried,
    OnOceanFloor,
    InMountain,
    Underground,
    InNether,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    pub placement: VerticalPlacement,
    pub air_pocket_probability: f32,
    pub mossiness: f32,
    pub overgrown: bool,
    pub vines: bool,
    pub can_be_cold: bool,
    pub replace_with_blackstone: bool,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    setups: Vec<Setup>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InitialSelection {
    pub setup_index: usize,
    pub air_pocket: bool,
    pub template: ResourceId,
    pub rotation: Rotation,
    pub mirror: Mirror,
}

/// Find the portal's Y using unadjusted noise columns, before biome validation.
/// `opaque` must use the requested worldgen heightmap predicate for each block.
/// On error the caller must discard the random state and any callback state.
pub fn find_suitable_y(
    random: &mut LegacyRandom,
    placement: VerticalPlacement,
    air_pocket: bool,
    surface_y: i32,
    piece_height: i32,
    bounds: BoundingBox,
    min_y: i32,
    mut opaque: impl FnMut([i32; 3], Heightmap) -> Result<bool>,
) -> Result<i32> {
    fn between(random: &mut LegacyRandom, min: i32, max: i32) -> Result<i32> {
        let width = max
            .checked_sub(min)
            .and_then(|value| value.checked_add(1))
            .context("invalid ruined portal height interval")?;
        Ok(min
            + random
                .next_int_bound(width)
                .context("invalid ruined portal height interval")?)
    }
    fn bounded(random: &mut LegacyRandom, min: i32, max: i32) -> Result<i32> {
        if min < max {
            between(random, min, max)
        } else {
            Ok(max)
        }
    }
    let bottom = min_y.wrapping_add(15);
    let top = surface_y.wrapping_sub(piece_height);
    let mut y = match placement {
        VerticalPlacement::InNether if air_pocket => between(random, 32, 100)?,
        VerticalPlacement::InNether if random.next_float() < 0.5 => between(random, 27, 29)?,
        VerticalPlacement::InNether => between(random, 29, 100)?,
        VerticalPlacement::InMountain => bounded(random, 70, top)?,
        VerticalPlacement::Underground => bounded(random, bottom, top)?,
        VerticalPlacement::PartlyBuried => top.wrapping_add(between(random, 2, 8)?),
        VerticalPlacement::OnLandSurface | VerticalPlacement::OnOceanFloor => surface_y,
    };
    let map = if placement == VerticalPlacement::OnOceanFloor {
        Heightmap::OceanFloor
    } else {
        Heightmap::WorldSurface
    };
    let corners = [
        [bounds.min[0], bounds.min[2]],
        [bounds.max[0], bounds.min[2]],
        [bounds.min[0], bounds.max[2]],
        [bounds.max[0], bounds.max[2]],
    ];
    while y > bottom {
        let mut solid = 0;
        for [x, z] in corners {
            if opaque([x, y, z], map)? {
                solid += 1;
                if solid == 3 {
                    return Ok(y);
                }
            }
        }
        y -= 1;
    }
    Ok(y)
}

impl Settings {
    pub fn setups(&self) -> &[Setup] {
        &self.setups
    }

    pub fn decode(value: &Value) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing structure type")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "ruined_portal",
            "not a ruined portal structure"
        );
        let values = value
            .get("setups")
            .and_then(Value::as_array)
            .context("missing ruined portal setups")?;
        ensure!(!values.is_empty(), "ruined portal setups are empty");
        let mut setups = Vec::with_capacity(values.len());
        for value in values {
            let placement = match value.get("placement").and_then(Value::as_str) {
                Some("on_land_surface") => VerticalPlacement::OnLandSurface,
                Some("partly_buried") => VerticalPlacement::PartlyBuried,
                Some("on_ocean_floor") => VerticalPlacement::OnOceanFloor,
                Some("in_mountain") => VerticalPlacement::InMountain,
                Some("underground") => VerticalPlacement::Underground,
                Some("in_nether") => VerticalPlacement::InNether,
                _ => anyhow::bail!("invalid ruined portal placement"),
            };
            let number = |name: &str| -> Result<f32> {
                let result = value
                    .get(name)
                    .and_then(Value::as_f64)
                    .with_context(|| format!("invalid ruined portal {name}"))?
                    as f32;
                ensure!(result.is_finite(), "invalid ruined portal {name}");
                Ok(result)
            };
            let boolean = |name: &str| -> Result<bool> {
                value
                    .get(name)
                    .and_then(Value::as_bool)
                    .with_context(|| format!("invalid ruined portal {name}"))
            };
            let air_pocket_probability = number("air_pocket_probability")?;
            let mossiness = number("mossiness")?;
            let weight = number("weight")?;
            ensure!(
                (0.0..=1.0).contains(&air_pocket_probability),
                "invalid air pocket probability"
            );
            ensure!((0.0..=1.0).contains(&mossiness), "invalid mossiness");
            ensure!(weight > 0.0, "invalid ruined portal weight");
            setups.push(Setup {
                placement,
                air_pocket_probability,
                mossiness,
                overgrown: boolean("overgrown")?,
                vines: boolean("vines")?,
                can_be_cold: boolean("can_be_cold")?,
                replace_with_blackstone: boolean("replace_with_blackstone")?,
                weight,
            });
        }
        Ok(Self { setups })
    }

    /// Consume the same random calls as Java through template rotation and mirror.
    /// The caller must still resolve the template, height, biome and piece data.
    /// Discard the random state if selection fails.
    pub fn select_initial(&self, random: &mut LegacyRandom) -> Result<InitialSelection> {
        ensure!(!self.setups.is_empty(), "ruined portal setups are empty");
        let setup_index = if self.setups.len() == 1 {
            0
        } else {
            let total = self
                .setups
                .iter()
                .fold(0.0_f32, |sum, setup| sum + setup.weight);
            ensure!(
                total.is_finite() && total > 0.0,
                "invalid ruined portal weight sum"
            );
            let mut choice = random.next_float();
            self.setups
                .iter()
                .position(|setup| {
                    choice -= setup.weight / total;
                    choice < 0.0
                })
                .context("ruined portal setup selection failed")?
        };
        let probability = self.setups[setup_index].air_pocket_probability;
        let air_pocket = if probability == 0.0 {
            false
        } else if probability == 1.0 {
            true
        } else {
            random.next_float() < probability
        };
        let giant = random.next_float() < 0.05;
        let number = random.next_int_bound(if giant { 3 } else { 10 }).unwrap() + 1;
        let template = ResourceId::parse(&format!(
            "minecraft:ruined_portal/{}portal_{number}",
            if giant { "giant_" } else { "" }
        ))?;
        let rotation = Rotation::random(random);
        let mirror = if random.next_float() < 0.5 {
            Mirror::None
        } else {
            Mirror::FrontBack
        };
        Ok(InitialSelection {
            setup_index,
            air_pocket,
            template,
            rotation,
            mirror,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup() -> Value {
        json!({"placement":"on_land_surface", "air_pocket_probability":0.35,
            "mossiness":0.2, "overgrown":false, "vines":false,
            "can_be_cold":true, "replace_with_blackstone":false, "weight":1.0})
    }

    #[test]
    fn validates_portal_settings() {
        let mut definition = json!({"type":"minecraft:ruined_portal", "setups":[setup()]});
        assert!(Settings::decode(&definition).is_ok());
        for (field, value) in [
            ("weight", json!(0)),
            ("mossiness", json!(1.1)),
            ("air_pocket_probability", json!(-0.1)),
            ("vines", json!("false")),
            ("placement", json!("unknown")),
        ] {
            let mut invalid = definition.clone();
            invalid["setups"][0][field] = value;
            assert!(Settings::decode(&invalid).is_err(), "{field}");
        }
        definition["setups"] = json!([]);
        assert!(Settings::decode(&definition).is_err());
    }

    #[test]
    fn certain_air_pockets_do_not_advance_random() {
        let mut never = setup();
        never["air_pocket_probability"] = json!(0.0);
        let mut always = setup();
        always["air_pocket_probability"] = json!(1.0);
        let never =
            Settings::decode(&json!({"type":"minecraft:ruined_portal", "setups":[never]})).unwrap();
        let always =
            Settings::decode(&json!({"type":"minecraft:ruined_portal", "setups":[always]}))
                .unwrap();
        for seed in 0..128 {
            let mut first = LegacyRandom::new(seed);
            let mut second = first.clone();
            assert!(!never.select_initial(&mut first).unwrap().air_pocket);
            assert!(always.select_initial(&mut second).unwrap().air_pocket);
            assert_eq!(first.state(), second.state());
        }
    }
}
