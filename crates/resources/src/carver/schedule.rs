//! Resolve biome carvers and combine their masks in Java's source-chunk order.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use super::{CarvingMask, canyon::CanyonCarver, cave::CaveCarver};
use crate::{
    packs::{ResourceId, ResourceProvider},
    recipes::{TagEntry, parse_tag},
};

#[derive(Clone)]
enum Carver {
    Cave(CaveCarver),
    Canyon(CanyonCarver),
}
impl Carver {
    fn decode(value: &Value) -> Result<Self> {
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing carver type")?,
        )?;
        let bytes = serde_json::to_vec(value)?;
        match (kind.namespace.as_str(), kind.path.as_str()) {
            ("minecraft", "cave") => Ok(Self::Cave(CaveCarver::decode(&bytes)?)),
            ("minecraft", "canyon") => Ok(Self::Canyon(CanyonCarver::decode(&bytes)?)),
            _ => bail!("unsupported carver type {}:{}", kind.namespace, kind.path),
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn carve(
        &self,
        seed: i64,
        index: i32,
        source: [i32; 2],
        target: [i32; 2],
        min_y: i32,
        height: i32,
        mask: &mut CarvingMask,
    ) -> Result<()> {
        match self {
            Self::Cave(c) => {
                c.carve_source(seed, index, source, target, min_y, height, mask)?;
            }
            Self::Canyon(c) => {
                c.carve_source(seed, index, source, target, min_y, height, mask)?;
            }
        }
        Ok(())
    }
}

/// Per-data-pack-stack cache. Discard it when the active packs change.
#[derive(Default)]
pub struct CarverCache {
    biomes: BTreeMap<ResourceId, Vec<Arc<Carver>>>,
    carvers: BTreeMap<ResourceId, Arc<Carver>>,
    tags: BTreeMap<ResourceId, Option<Vec<ResourceId>>>,
}
impl CarverCache {
    fn load_named(
        &mut self,
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
    ) -> Result<Option<Arc<Carver>>> {
        if let Some(carver) = self.carvers.get(id) {
            return Ok(Some(Arc::clone(carver)));
        }
        let path = format!("data/{}/worldgen/carver/{}.json", id.namespace, id.path);
        let Some(bytes) = provider.read_data(&path)? else {
            return Ok(None);
        };
        let value: Value =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid carver {path}"))?;
        let result = Arc::new(Carver::decode(&value).with_context(|| format!("in carver {path}"))?);
        self.carvers.insert(id.clone(), Arc::clone(&result));
        Ok(Some(result))
    }
    fn resolve_tag(
        &mut self,
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        visiting: &mut BTreeSet<ResourceId>,
    ) -> Result<Option<Vec<ResourceId>>> {
        if let Some(cached) = self.tags.get(id) {
            return Ok(cached.clone());
        }
        ensure!(
            visiting.len() < 128 && self.tags.len() < 4096,
            "carver tag nesting or count limit exceeded"
        );
        ensure!(visiting.insert(id.clone()), "cyclic carver tag reference");
        let result = (|| {
            let path = format!(
                "data/{}/tags/worldgen/carver/{}.json",
                id.namespace, id.path
            );
            let stack = provider.read_data_stack(&path)?;
            if stack.is_empty() {
                return Ok(None);
            }
            let mut entries = Vec::new();
            for bytes in stack {
                let (replace, next) =
                    parse_tag(&bytes).with_context(|| format!("invalid carver tag {path}"))?;
                if replace {
                    entries.clear();
                }
                entries.extend(next);
            }
            let mut result = Vec::new();
            let mut seen = BTreeSet::new();
            for entry in entries {
                match entry {
                    TagEntry::Item { id, required } => {
                        let id = ResourceId::parse(&id)?;
                        if self.load_named(provider, &id)?.is_some() {
                            if seen.insert(id.clone()) {
                                result.push(id);
                            }
                        } else if required {
                            bail!("required carver {}:{} is missing", id.namespace, id.path);
                        }
                    }
                    TagEntry::Tag { id, required } => {
                        match self.resolve_tag(provider, &id, visiting)? {
                            Some(values) => {
                                for item in values {
                                    if seen.insert(item.clone()) {
                                        result.push(item);
                                    }
                                }
                            }
                            None if required => bail!(
                                "required carver tag {}:{} is missing",
                                id.namespace,
                                id.path
                            ),
                            None => {}
                        }
                    }
                }
            }
            Ok(Some(result))
        })();
        visiting.remove(id);
        if let Ok(value) = &result {
            self.tags.insert(id.clone(), value.clone());
        }
        result
    }
    fn biome(
        &mut self,
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
    ) -> Result<&[Arc<Carver>]> {
        if !self.biomes.contains_key(id) {
            let path = format!("data/{}/worldgen/biome/{}.json", id.namespace, id.path);
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing biome {path}"))?;
            let biome: Value =
                serde_json::from_slice(&bytes).with_context(|| format!("invalid biome {path}"))?;
            let list = biome.get("carvers").context("biome missing carvers")?;
            let mut carvers = Vec::new();
            if let Some(tag) = list.as_str().and_then(|s| s.strip_prefix('#')) {
                let id = ResourceId::parse(tag)?;
                let ids = self
                    .resolve_tag(provider, &id, &mut BTreeSet::new())?
                    .context("missing carver tag")?;
                for id in ids {
                    carvers.push(
                        self.load_named(provider, &id)?
                            .context("missing tagged carver")?,
                    );
                }
            } else {
                for entry in list.as_array().context("carvers must be a list or tag")? {
                    if let Some(name) = entry.as_str() {
                        let id = ResourceId::parse(name)?;
                        carvers.push(
                            self.load_named(provider, &id)?
                                .with_context(|| format!("missing carver {name}"))?,
                        );
                    } else {
                        carvers.push(Arc::new(Carver::decode(entry)?));
                    }
                }
            }
            self.biomes.insert(id.clone(), carvers);
        }
        Ok(self.biomes.get(id).unwrap())
    }
    /// For each source chunk, query its noise biome at the origin quart and
    /// quart Y=0; the callback must use the active dimension's biome source.
    #[allow(clippy::too_many_arguments)]
    pub fn generate(
        &mut self,
        provider: &mut impl ResourceProvider,
        seed: i64,
        target: [i32; 2],
        min_y: i32,
        height: i32,
        mut biome: impl FnMut([i32; 2]) -> Result<ResourceId>,
    ) -> Result<CarvingMask> {
        let top = min_y
            .checked_add(height)
            .and_then(|y| y.checked_sub(8))
            .context("invalid carver height")?;
        let bottom = min_y.checked_add(1).context("invalid carver height")?;
        let mut mask = CarvingMask::new(bottom, top)?;
        for dx in -8..=8 {
            for dz in -8..=8 {
                let source = [
                    target[0]
                        .checked_add(dx)
                        .context("source chunk X overflow")?,
                    target[1]
                        .checked_add(dz)
                        .context("source chunk Z overflow")?,
                ];
                let id = biome(source)?;
                for (index, carver) in self.biome(provider, &id)?.iter().enumerate() {
                    carver.carve(
                        seed,
                        i32::try_from(index)?,
                        source,
                        target,
                        min_y,
                        height,
                        &mut mask,
                    )?;
                }
            }
        }
        Ok(mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[derive(Default)]
    struct Data(BTreeMap<String, Vec<Vec<u8>>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).and_then(|v| v.last()).cloned())
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
    }
    #[test]
    fn inline_carvers_and_invalid_tag_references_are_handled_explicitly() {
        let mut data = Data::default();
        let config = json!({"type":"cave","probability":1,"y":{"absolute":0},"count":0,"thickness":1,
          "floor_level":0,"room_vertical_radius_multiplier":1,"horizontal_radius_multiplier":1,"vertical_radius_multiplier":1});
        data.0.insert(
            "data/test/worldgen/biome/inline.json".into(),
            vec![serde_json::to_vec(&json!({"carvers":[config]})).unwrap()],
        );
        data.0.insert(
            "data/test/worldgen/biome/cycle.json".into(),
            vec![br##"{"carvers":"#test:cycle"}"##.to_vec()],
        );
        data.0.insert(
            "data/test/tags/worldgen/carver/cycle.json".into(),
            vec![br##"{"values":["#test:cycle"]}"##.to_vec()],
        );
        let mut cache = CarverCache::default();
        let mask = cache
            .generate(&mut data, 0, [0, 0], 0, 16, |_| {
                ResourceId::parse("test:inline")
            })
            .unwrap();
        assert!(mask.is_empty());
        assert!(
            cache
                .generate(&mut data, 0, [0, 0], 0, 16, |_| ResourceId::parse(
                    "test:cycle"
                ))
                .is_err()
        );
        assert!(
            cache
                .generate(&mut data, 0, [0, 0], 0, 16, |_| ResourceId::parse(
                    "test:missing"
                ))
                .is_err()
        );
        assert!(
            cache
                .generate(&mut data, 0, [0, 0], 0, 8, |_| ResourceId::parse(
                    "test:inline"
                ))
                .is_err()
        );
    }
}
