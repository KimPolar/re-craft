//! Structure registry order and per-step seeds for chunk decoration.
//!
//! This schedules registry entries only; it does not validate structure
//! codecs, discover starts, place blocks, or authorize world persistence.
use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;
use std::collections::BTreeSet;

use crate::packs::{ResourceId, ResourceProvider};

const STEPS: [&str; 11] = [
    "raw_generation",
    "lakes",
    "local_modifications",
    "underground_structures",
    "surface_structures",
    "strongholds",
    "underground_ores",
    "underground_decoration",
    "fluid_springs",
    "vegetal_decoration",
    "top_layer_modification",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructureDecorationEntry {
    pub id: ResourceId,
    pub step: i32,
    pub index: i32,
}

impl StructureDecorationEntry {
    /// Java reseeds once per registered structure, then shares this random
    /// stream between all referenced starts for that structure in this chunk.
    pub fn random(&self, decoration_seed: i64) -> DecorationRandom {
        let mut random = DecorationRandom::new(0);
        random.set_feature_seed(decoration_seed, self.index, self.step);
        random
    }
}

#[derive(Clone, Debug)]
pub struct StructureDecorationSchedule {
    steps: Vec<Vec<StructureDecorationEntry>>,
}

impl StructureDecorationSchedule {
    /// Load every active registry entry in resource-file registration order.
    /// An omitted structure must not silently change later RNG indices.
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let ids = provider.list_registry_data("worldgen/structure")?;
        let mut seen = BTreeSet::new();
        let mut steps = vec![Vec::new(); STEPS.len()];
        for id in ids {
            ensure!(
                seen.insert(id.clone()),
                "duplicate structure registry entry"
            );
            let path = format!("data/{}/worldgen/structure/{}.json", id.namespace, id.path);
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing structure {path}"))?;
            let value: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid structure {path}"))?;
            let name = value
                .get("step")
                .and_then(Value::as_str)
                .with_context(|| format!("structure step missing in {path}"))?;
            let step = STEPS
                .iter()
                .position(|&candidate| candidate == name)
                .with_context(|| format!("invalid structure step {name} in {path}"))?;
            let index = i32::try_from(steps[step].len())
                .context("too many structures in one decoration step")?;
            steps[step].push(StructureDecorationEntry {
                id,
                step: step as i32,
                index,
            });
        }
        Ok(Self { steps })
    }

    pub fn steps(&self) -> &[Vec<StructureDecorationEntry>] {
        &self.steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Data(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
        fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
            Ok(vec![
                ResourceId::parse("z:z").unwrap(),
                ResourceId::parse("z:a").unwrap(),
                ResourceId::parse("a:a").unwrap(),
            ])
        }
    }

    #[test]
    fn registry_order_sets_step_indices_and_invalid_steps_fail() {
        let mut data = Data(BTreeMap::from([
            (
                "data/a/worldgen/structure/a.json".into(),
                br#"{"step":"surface_structures"}"#.to_vec(),
            ),
            (
                "data/z/worldgen/structure/a.json".into(),
                br#"{"step":"underground_structures"}"#.to_vec(),
            ),
            (
                "data/z/worldgen/structure/z.json".into(),
                br#"{"step":"surface_structures"}"#.to_vec(),
            ),
        ]));
        let schedule = StructureDecorationSchedule::load(&mut data).unwrap();
        assert_eq!(schedule.steps()[3][0].id, ResourceId::parse("z:a").unwrap());
        assert_eq!(
            schedule.steps()[4]
                .iter()
                .map(|entry| (&entry.id, entry.index))
                .collect::<Vec<_>>(),
            vec![
                (&ResourceId::parse("a:a").unwrap(), 0),
                (&ResourceId::parse("z:z").unwrap(), 1)
            ]
        );
        data.0.insert(
            "data/z/worldgen/structure/z.json".into(),
            br#"{"step":"unknown"}"#.to_vec(),
        );
        assert!(StructureDecorationSchedule::load(&mut data).is_err());
        data.0.remove("data/z/worldgen/structure/z.json");
        assert!(StructureDecorationSchedule::load(&mut data).is_err());
    }
}
