//! Structure-set candidate chunks for 26.3 random-spread placements.
//! Candidate placement does not mean a structure start is valid or generated.
use crate::packs::{ResourceId, ResourceProvider};
use anyhow::{Context, Result, bail, ensure};
use native_core::random::LegacyRandom;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Spread {
    Linear,
    Triangular,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Reduction {
    Default,
    Legacy1,
    Legacy2,
    Legacy3,
}
#[derive(Clone, Debug)]
pub struct RandomSpreadPlacement {
    spacing: i32,
    separation: i32,
    salt: i32,
    frequency: f32,
    spread: Spread,
    reduction: Reduction,
    locate_offset: [i32; 3],
    exclusion: Option<(ResourceId, i32)>,
}
fn integer(value: &Value, name: &str) -> Result<i32> {
    let v = value
        .get(name)
        .and_then(Value::as_i64)
        .with_context(|| format!("missing or invalid {name}"))?;
    Ok(i32::try_from(v)?)
}
fn salted(seed: i64, x: i32, z: i32, salt: i32) -> LegacyRandom {
    LegacyRandom::new(
        i64::from(x)
            .wrapping_mul(341_873_128_712)
            .wrapping_add(i64::from(z).wrapping_mul(132_897_987_541))
            .wrapping_add(seed)
            .wrapping_add(i64::from(salt)),
    )
}
fn large_feature(seed: i64, x: i32, z: i32) -> LegacyRandom {
    let mut r = LegacyRandom::new(seed);
    let a = r.next_long();
    let b = r.next_long();
    LegacyRandom::new(i64::from(x).wrapping_mul(a) ^ i64::from(z).wrapping_mul(b) ^ seed)
}
impl RandomSpreadPlacement {
    pub fn decode(value: &Value) -> Result<Self> {
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing placement type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "random_spread",
            "unsupported structure placement {}:{}",
            kind.namespace,
            kind.path
        );
        let spacing = integer(value, "spacing")?;
        let separation = integer(value, "separation")?;
        let salt = integer(value, "salt")?;
        ensure!(
            (0..=4096).contains(&spacing)
                && (0..=4096).contains(&separation)
                && spacing > separation
                && salt >= 0,
            "invalid structure spacing, separation or salt"
        );
        let frequency = value
            .get("frequency")
            .map(|v| v.as_f64().context("invalid frequency"))
            .transpose()?
            .unwrap_or(1.0) as f32;
        ensure!(
            frequency.is_finite() && (0.0..=1.0).contains(&frequency),
            "invalid structure frequency"
        );
        let spread = match value
            .get("spread_type")
            .map(|v| v.as_str().context("invalid spread type"))
            .transpose()?
            .unwrap_or("linear")
        {
            "linear" => Spread::Linear,
            "triangular" => Spread::Triangular,
            name => bail!("unsupported spread type {name}"),
        };
        let reduction = match value
            .get("frequency_reduction_method")
            .map(|v| v.as_str().context("invalid frequency reduction"))
            .transpose()?
            .unwrap_or("default")
        {
            "default" => Reduction::Default,
            "legacy_type_1" => Reduction::Legacy1,
            "legacy_type_2" => Reduction::Legacy2,
            "legacy_type_3" => Reduction::Legacy3,
            name => bail!("unsupported frequency reduction {name}"),
        };
        let mut locate_offset = [0; 3];
        if let Some(offset) = value.get("locate_offset") {
            let offset = offset.as_array().context("invalid locate offset")?;
            ensure!(offset.len() == 3, "invalid locate offset");
            for i in 0..3 {
                locate_offset[i] =
                    i32::try_from(offset[i].as_i64().context("invalid locate offset")?)?;
                ensure!(
                    (-16..=16).contains(&locate_offset[i]),
                    "locate offset out of range"
                );
            }
        }
        let exclusion = value
            .get("exclusion_zone")
            .map(|zone| -> Result<_> {
                let name = zone
                    .get("other_set")
                    .and_then(Value::as_str)
                    .context("inline or invalid exclusion structure set is unsupported")?;
                let range = integer(zone, "chunk_count")?;
                ensure!((1..=16).contains(&range), "invalid exclusion range");
                Ok((ResourceId::parse(name)?, range))
            })
            .transpose()?;
        Ok(Self {
            spacing,
            separation,
            salt,
            frequency,
            spread,
            reduction,
            locate_offset,
            exclusion,
        })
    }
    /// The cell's deterministic candidate, prior to frequency and exclusions.
    pub fn potential_chunk(&self, seed: i64, source: [i32; 2]) -> [i32; 2] {
        let [x, z] = source;
        let gx = x.div_euclid(self.spacing);
        let gz = z.div_euclid(self.spacing);
        let mut random = salted(seed, gx, gz, self.salt);
        let limit = self.spacing - self.separation;
        let mut offset = || {
            let a = random.next_int_bound(limit).unwrap();
            if self.spread == Spread::Linear {
                a
            } else {
                (a + random.next_int_bound(limit).unwrap()) / 2
            }
        };
        [
            gx.wrapping_mul(self.spacing).wrapping_add(offset()),
            gz.wrapping_mul(self.spacing).wrapping_add(offset()),
        ]
    }
    pub fn passes_frequency(&self, seed: i64, source: [i32; 2]) -> bool {
        if self.frequency >= 1.0 {
            return true;
        }
        let [x, z] = source;
        match self.reduction {
            Reduction::Default => salted(seed, self.salt, x, z).next_float() < self.frequency,
            Reduction::Legacy1 => {
                let cx = x >> 4;
                let cz = z >> 4;
                let mut r = LegacyRandom::new(i64::from(cx ^ (cz.wrapping_shl(4))) ^ seed);
                r.next_int();
                r.next_int_bound((1.0f32 / self.frequency) as i32).unwrap() == 0
            }
            Reduction::Legacy2 => salted(seed, x, z, 10_387_320).next_float() < self.frequency,
            Reduction::Legacy3 => {
                large_feature(seed, x, z).next_double() < f64::from(self.frequency)
            }
        }
    }
    pub fn is_candidate_before_exclusion(&self, seed: i64, source: [i32; 2]) -> bool {
        self.potential_chunk(seed, source) == source && self.passes_frequency(seed, source)
    }
    pub fn locate_position(&self, chunk: [i32; 2]) -> [i32; 3] {
        [
            chunk[0]
                .wrapping_mul(16)
                .wrapping_add(self.locate_offset[0]),
            self.locate_offset[1],
            chunk[1]
                .wrapping_mul(16)
                .wrapping_add(self.locate_offset[2]),
        ]
    }
}

#[derive(Clone, Debug)]
pub struct StructureEntry {
    pub id: ResourceId,
    pub weight: i32,
}
#[derive(Clone, Debug)]
pub struct RandomSpreadSet {
    pub placement: RandomSpreadPlacement,
    pub structures: Vec<StructureEntry>,
}
impl RandomSpreadSet {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(bytes)?;
        let placement = RandomSpreadPlacement::decode(
            value
                .get("placement")
                .context("missing structure placement")?,
        )?;
        let array = value
            .get("structures")
            .and_then(Value::as_array)
            .context("invalid structure list")?;
        let mut structures = Vec::with_capacity(array.len());
        let mut total = 0i32;
        for entry in array {
            let id = entry
                .get("structure")
                .and_then(Value::as_str)
                .context("inline or invalid structure is unsupported")?;
            let weight = integer(entry, "weight")?;
            ensure!(weight > 0, "invalid structure weight");
            total = total
                .checked_add(weight)
                .context("structure weight total overflow")?;
            structures.push(StructureEntry {
                id: ResourceId::parse(id)?,
                weight,
            });
        }
        Ok(Self {
            placement,
            structures,
        })
    }
    /// Order in which Java tries structures if preceding attempts fail.
    pub fn attempt_order(&self, seed: i64, chunk: [i32; 2]) -> Vec<ResourceId> {
        if self.structures.len() < 2 {
            return self.structures.iter().map(|v| v.id.clone()).collect();
        }
        let mut options = self.structures.clone();
        let mut total: i32 = options.iter().map(|v| v.weight).sum();
        let mut random = large_feature(seed, chunk[0], chunk[1]);
        let mut result = Vec::with_capacity(options.len());
        while !options.is_empty() {
            let mut choice = random.next_int_bound(total).unwrap();
            let mut index = 0;
            for v in &options {
                choice -= v.weight;
                if choice < 0 {
                    break;
                }
                index += 1;
            }
            let chosen = options.remove(index);
            total -= chosen.weight;
            result.push(chosen.id);
        }
        result
    }
}

/// Cache belongs to one active data-pack stack and one generator configuration.
#[derive(Default)]
pub struct StructureSetCache {
    sets: BTreeMap<ResourceId, RandomSpreadSet>,
}
impl StructureSetCache {
    pub fn load<'a>(
        &'a mut self,
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
    ) -> Result<&'a RandomSpreadSet> {
        if !self.sets.contains_key(id) {
            let path = format!(
                "data/{}/worldgen/structure_set/{}.json",
                id.namespace, id.path
            );
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing structure set {path}"))?;
            let set = RandomSpreadSet::decode(&bytes)
                .with_context(|| format!("in structure set {path}"))?;
            self.sets.insert(id.clone(), set);
        }
        Ok(self.sets.get(id).unwrap())
    }
    pub fn is_structure_chunk(
        &mut self,
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<bool> {
        self.resolve(provider, id, seed, chunk, &mut BTreeSet::new())
    }
    fn resolve(
        &mut self,
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        seed: i64,
        chunk: [i32; 2],
        visiting: &mut BTreeSet<ResourceId>,
    ) -> Result<bool> {
        ensure!(
            visiting.len() < 128 && visiting.insert(id.clone()),
            "cyclic structure exclusion"
        );
        let result = (|| {
            let placement = self.load(provider, id)?.placement.clone();
            if !placement.is_candidate_before_exclusion(seed, chunk) {
                return Ok(false);
            }
            if let Some((other, range)) = placement.exclusion {
                for x in chunk[0].wrapping_sub(range)..=chunk[0].wrapping_add(range) {
                    for z in chunk[1].wrapping_sub(range)..=chunk[1].wrapping_add(range) {
                        if self.resolve(provider, &other, seed, [x, z], visiting)? {
                            return Ok(false);
                        }
                    }
                }
            }
            Ok(true)
        })();
        visiting.remove(id);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[derive(Default)]
    struct Data(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }
    fn set() -> Value {
        json!({"placement":{"type":"random_spread","spacing":1,"separation":0,"salt":0},"structures":[{"structure":"minecraft:test","weight":1}]})
    }
    #[test]
    fn rejects_invalid_settings_and_cyclic_exclusions() {
        let mut value = set();
        value["placement"]["spacing"] = json!(0);
        assert!(RandomSpreadSet::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        value = set();
        value["placement"]["locate_offset"] = json!([17, 0, 0]);
        assert!(RandomSpreadSet::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        value = set();
        value["structures"] = json!([{"structure":"minecraft:test","weight":2147483647},{"structure":"minecraft:test","weight":1}]);
        assert!(RandomSpreadSet::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut a = set();
        a["placement"]["exclusion_zone"] = json!({"other_set":"test:b","chunk_count":1});
        let mut b = set();
        b["placement"]["exclusion_zone"] = json!({"other_set":"test:a","chunk_count":1});
        let mut data = Data(BTreeMap::from([
            (
                "data/test/worldgen/structure_set/a.json".into(),
                serde_json::to_vec(&a).unwrap(),
            ),
            (
                "data/test/worldgen/structure_set/b.json".into(),
                serde_json::to_vec(&b).unwrap(),
            ),
        ]));
        let mut cache = StructureSetCache::default();
        assert!(
            cache
                .is_structure_chunk(&mut data, &ResourceId::parse("test:a").unwrap(), 0, [0, 0])
                .is_err()
        );
        assert!(
            cache
                .load(&mut data, &ResourceId::parse("test:missing").unwrap())
                .is_err()
        );
    }
}
