//! Data-pack template pools and Java's weighted selection order for jigsaws.
//!
//! This module retains template, feature and processor definitions; placing
//! pieces, resolving holders and applying processors remain separate stages.
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Projection {
    Rigid,
    TerrainMatching,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiquidSettings {
    IgnoreWaterlogging,
    ApplyWaterlogging,
}

/// A registry reference or a data-pack-supplied inline definition.
#[derive(Clone, Debug, PartialEq)]
pub enum HolderValue {
    Reference(ResourceId),
    Inline(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PoolElement {
    Empty,
    Single {
        legacy: bool,
        location: ResourceId,
        processors: HolderValue,
        projection: Projection,
        override_liquid_settings: Option<LiquidSettings>,
    },
    Feature {
        feature: HolderValue,
        projection: Projection,
    },
    List {
        elements: Vec<PoolElement>,
        projection: Projection,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PoolFallback {
    Reference(ResourceId),
    Inline(Box<TemplatePool>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemplatePool {
    pub fallback: PoolFallback,
    entries: Vec<(PoolElement, u8)>,
    total_weight: i32,
}

fn holder(value: &Value, label: &str) -> Result<HolderValue> {
    match value {
        Value::String(id) => Ok(HolderValue::Reference(ResourceId::parse(id)?)),
        Value::Object(_) => Ok(HolderValue::Inline(value.clone())),
        _ => bail!("invalid {label}: expected a reference or inline object"),
    }
}

fn projection(value: &Value) -> Result<Projection> {
    match value.get("projection").and_then(Value::as_str) {
        Some("rigid") => Ok(Projection::Rigid),
        Some("terrain_matching") => Ok(Projection::TerrainMatching),
        _ => bail!("missing or invalid template pool projection"),
    }
}

impl PoolElement {
    fn decode(value: &Value, depth: usize) -> Result<Self> {
        ensure!(depth <= 64, "template pool nesting is too deep");
        let kind = value
            .get("element_type")
            .and_then(Value::as_str)
            .context("missing pool element type")?;
        let kind = ResourceId::parse(kind)?;
        ensure!(
            kind.namespace == "minecraft",
            "unsupported pool element namespace"
        );
        Ok(match kind.path.as_str() {
            "empty_pool_element" => Self::Empty,
            "single_pool_element" | "legacy_single_pool_element" => Self::Single {
                legacy: kind.path == "legacy_single_pool_element",
                location: ResourceId::parse(
                    value
                        .get("location")
                        .and_then(Value::as_str)
                        .context("missing template location")?,
                )?,
                processors: holder(
                    value
                        .get("processors")
                        .context("missing template processors")?,
                    "template processors",
                )?,
                projection: projection(value)?,
                override_liquid_settings: match value.get("override_liquid_settings") {
                    None => None,
                    Some(Value::String(name)) if name == "ignore_waterlogging" => {
                        Some(LiquidSettings::IgnoreWaterlogging)
                    }
                    Some(Value::String(name)) if name == "apply_waterlogging" => {
                        Some(LiquidSettings::ApplyWaterlogging)
                    }
                    Some(_) => bail!("invalid override liquid settings"),
                },
            },
            "feature_pool_element" => Self::Feature {
                feature: holder(
                    value.get("feature").context("missing pool feature")?,
                    "pool feature",
                )?,
                projection: projection(value)?,
            },
            "list_pool_element" => {
                let values = value
                    .get("elements")
                    .and_then(Value::as_array)
                    .context("missing pool element list")?;
                ensure!(!values.is_empty(), "pool element list is empty");
                let projection = projection(value)?;
                let mut elements = values
                    .iter()
                    .map(|item| Self::decode(item, depth + 1))
                    .collect::<Result<Vec<_>>>()?;
                for element in &mut elements {
                    element.set_projection(projection);
                }
                Self::List {
                    elements,
                    projection,
                }
            }
            _ => bail!(
                "unsupported pool element type {}:{}",
                kind.namespace,
                kind.path
            ),
        })
    }

    fn set_projection(&mut self, value: Projection) {
        match self {
            Self::Empty => {}
            Self::Single { projection, .. } | Self::Feature { projection, .. } => {
                *projection = value
            }
            Self::List {
                elements,
                projection,
            } => {
                *projection = value;
                for element in elements {
                    element.set_projection(value);
                }
            }
        }
    }
}

impl TemplatePool {
    pub fn load(provider: &mut impl ResourceProvider, id: &ResourceId) -> Result<Self> {
        Self::load_optional(provider, id)?
            .with_context(|| format!("missing template pool {}:{}", id.namespace, id.path))
    }

    /// Return None for a missing pool, as distinct from invalid pool data.
    pub fn load_optional(
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
    ) -> Result<Option<Self>> {
        let checked = ResourceId::parse(&format!("{}:{}", id.namespace, id.path))?;
        ensure!(&checked == id, "invalid template pool identifier");
        let path = format!(
            "data/{}/worldgen/template_pool/{}.json",
            id.namespace, id.path
        );
        let Some(bytes) = provider.read_data(&path)? else {
            return Ok(None);
        };
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid template pool {path}"))?;
        Self::decode(&value)
            .with_context(|| format!("in template pool {path}"))
            .map(Some)
    }

    pub fn decode(value: &Value) -> Result<Self> {
        Self::decode_nested(value, 0)
    }

    fn decode_nested(value: &Value, depth: usize) -> Result<Self> {
        ensure!(depth <= 64, "template pool nesting is too deep");
        let fallback = match value
            .get("fallback")
            .context("missing template pool fallback")?
        {
            Value::String(id) => PoolFallback::Reference(ResourceId::parse(id)?),
            value @ Value::Object(_) => {
                PoolFallback::Inline(Box::new(Self::decode_nested(value, depth + 1)?))
            }
            _ => bail!("invalid template pool fallback"),
        };
        let entries = value
            .get("elements")
            .and_then(Value::as_array)
            .context("missing template pool elements")?;
        let mut decoded = Vec::with_capacity(entries.len());
        let mut total_weight = 0_i32;
        for entry in entries {
            let weight = entry
                .get("weight")
                .and_then(Value::as_i64)
                .context("missing pool element weight")?;
            ensure!(
                (1..=150).contains(&weight),
                "pool element weight out of range"
            );
            total_weight = total_weight
                .checked_add(weight as i32)
                .context("template pool weight overflow")?;
            let element = PoolElement::decode(
                entry.get("element").context("missing pool element")?,
                depth + 1,
            )?;
            decoded.push((element, weight as u8));
        }
        Ok(Self {
            fallback,
            entries: decoded,
            total_weight,
        })
    }

    pub fn entries(&self) -> &[(PoolElement, u8)] {
        &self.entries
    }
    pub fn weighted_len(&self) -> i32 {
        self.total_weight
    }

    /// Select exactly one weighted entry; an empty pool returns Java's empty
    /// sentinel without consuming the random source.
    pub fn random_element(
        &self,
        random: &mut impl FnMut(i32) -> Option<i32>,
    ) -> Result<Option<&PoolElement>> {
        if self.total_weight == 0 {
            return Ok(None);
        }
        let mut index = random(self.total_weight).context("invalid pool random bound")?;
        ensure!(
            (0..self.total_weight).contains(&index),
            "pool random result out of bounds"
        );
        for (element, weight) in &self.entries {
            if index < i32::from(*weight) {
                return Ok(Some(element));
            }
            index -= i32::from(*weight);
        }
        unreachable!("pool weights were validated")
    }

    /// Java expands weighted entries in input order, then performs a reverse
    /// Fisher–Yates shuffle, drawing once for every length from N down to 2.
    pub fn shuffled_elements(
        &self,
        random: &mut impl FnMut(i32) -> Option<i32>,
    ) -> Result<Vec<&PoolElement>> {
        let mut result = Vec::new();
        result
            .try_reserve_exact(self.total_weight as usize)
            .context("template pool is too large")?;
        for (element, weight) in &self.entries {
            result.extend(std::iter::repeat_n(element, usize::from(*weight)));
        }
        for end in (2..=self.total_weight).rev() {
            let index = random(end).context("invalid pool shuffle bound")?;
            ensure!(
                (0..end).contains(&index),
                "pool shuffle result out of bounds"
            );
            result.swap((end - 1) as usize, index as usize);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> TemplatePool {
        TemplatePool::decode(&json!({
            "fallback":"minecraft:empty",
            "elements":[
                {"weight":2,"element":{"element_type":"minecraft:single_pool_element","location":"example:a","processors":"minecraft:empty","projection":"rigid"}},
                {"weight":3,"element":{"element_type":"minecraft:single_pool_element","location":"example:b","processors":"minecraft:empty","projection":"rigid"}},
                {"weight":1,"element":{"element_type":"minecraft:empty_pool_element"}}
            ]
        })).unwrap()
    }

    fn label(element: &PoolElement) -> char {
        match element {
            PoolElement::Single { location, .. } if location.path == "a" => 'a',
            PoolElement::Single { location, .. } if location.path == "b" => 'b',
            PoolElement::Empty => 'c',
            _ => panic!("unexpected pool element"),
        }
    }

    #[test]
    fn selection_expands_weights_and_preserves_empty_pool_rng() -> Result<()> {
        let pool = fixture();
        assert_eq!(pool.weighted_len(), 6);
        for (index, expected) in ['a', 'a', 'b', 'b', 'b', 'c'].into_iter().enumerate() {
            assert_eq!(
                label(pool.random_element(&mut |_| Some(index as i32))?.unwrap()),
                expected
            );
        }
        let mut bounds = Vec::new();
        let shuffled = pool.shuffled_elements(&mut |bound| {
            bounds.push(bound);
            Some(0)
        })?;
        assert_eq!(bounds, [6, 5, 4, 3, 2]);
        assert_eq!(
            shuffled.into_iter().map(label).collect::<String>(),
            "abbbca"
        );
        let empty = TemplatePool::decode(&json!({"fallback":"minecraft:empty","elements":[]}))?;
        let mut no_draw = |_| panic!("empty pool consumed random input");
        assert!(empty.random_element(&mut no_draw)?.is_none());
        assert!(empty.shuffled_elements(&mut no_draw)?.is_empty());
        assert!(pool.random_element(&mut |bound| Some(bound)).is_err());
        Ok(())
    }

    #[test]
    fn validates_pool_and_propagates_list_projection() -> Result<()> {
        let value = json!({"fallback":{"fallback":"minecraft:empty","elements":[]},"elements":[
            {"weight":1,"element":{"element_type":"minecraft:list_pool_element","projection":"terrain_matching","elements":[
                {"element_type":"minecraft:single_pool_element","location":"example:room","processors":{"processors":[]},"projection":"rigid","override_liquid_settings":"ignore_waterlogging"},
                {"element_type":"minecraft:feature_pool_element","feature":{"feature":"example:test"},"projection":"rigid"}
            ]}}
        ]});
        let pool = TemplatePool::decode(&value)?;
        assert!(matches!(pool.fallback, PoolFallback::Inline(_)));
        let PoolElement::List {
            elements,
            projection,
        } = &pool.entries()[0].0
        else {
            panic!("expected list")
        };
        assert_eq!(*projection, Projection::TerrainMatching);
        assert!(matches!(
            &elements[0],
            PoolElement::Single {
                projection: Projection::TerrainMatching,
                override_liquid_settings: Some(LiquidSettings::IgnoreWaterlogging),
                processors: HolderValue::Inline(_),
                ..
            }
        ));
        assert!(matches!(
            &elements[1],
            PoolElement::Feature {
                projection: Projection::TerrainMatching,
                ..
            }
        ));
        for weight in [0, 151, -1] {
            let mut invalid = value.clone();
            invalid["elements"][0]["weight"] = json!(weight);
            assert!(TemplatePool::decode(&invalid).is_err());
        }
        let mut invalid = value.clone();
        invalid["elements"][0]["element"]["elements"] = json!([]);
        assert!(TemplatePool::decode(&invalid).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn all_vanilla_template_pools_decode() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut resources = crate::packs::ResourceStack::open(&game)?;
        let ids = resources.list_data("worldgen/template_pool")?;
        ensure!(ids.len() > 100, "missing vanilla template pools");
        for id in ids {
            TemplatePool::load(&mut resources, &id)?;
        }
        // An active data pack may override a vanilla pool and add a pool in
        // its own namespace. Read through the same stack used by generation.
        let world = tempfile::tempdir()?;
        let pack = world.path().join("datapacks/custom");
        std::fs::create_dir_all(pack.join("data/minecraft/worldgen/template_pool/ancient_city"))?;
        std::fs::create_dir_all(pack.join("data/example/worldgen/template_pool"))?;
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )?;
        let override_pool = json!({"fallback":"minecraft:empty","elements":[
            {"weight":7,"element":{"element_type":"minecraft:empty_pool_element"}}
        ]});
        std::fs::write(
            pack.join("data/minecraft/worldgen/template_pool/ancient_city/city_center.json"),
            serde_json::to_vec(&override_pool)?,
        )?;
        std::fs::write(
            pack.join("data/example/worldgen/template_pool/custom.json"),
            serde_json::to_vec(&override_pool)?,
        )?;
        let selection = crate::datapacks::DataPackSettings {
            enabled: vec!["vanilla".into(), "file/custom".into()],
            disabled: vec![],
            enabled_features: vec!["minecraft:vanilla".into()],
        };
        let warnings = resources.configure_world_data(world.path(), &selection)?;
        ensure!(
            warnings.is_empty(),
            "data pack selection failed: {warnings:?}"
        );
        for name in ["minecraft:ancient_city/city_center", "example:custom"] {
            let loaded = TemplatePool::load(&mut resources, &ResourceId::parse(name)?)?;
            assert_eq!(loaded.weighted_len(), 7);
            assert!(matches!(loaded.entries()[0].0, PoolElement::Empty));
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn weighted_selection_and_shuffle_match_java() -> Result<()> {
        use native_core::random::{DecorationRandom, LegacyRandom};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let _verified = crate::packs::ResourceStack::open(&game)?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home is missing")?
            .join("bin");
        let source = temp.path().join("JigsawPoolOracle.java");
        std::fs::write(&source, include_str!("../tests/java/JigsawPoolOracle.java"))?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawPoolOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let pool = fixture();
        let mut count = 0;
        for line in String::from_utf8(output.stdout)?
            .lines()
            .filter_map(|line| line.strip_prefix("P\t"))
        {
            let values: Vec<_> = line.split('\t').collect();
            ensure!(values.len() == 5, "invalid Java pool output");
            let seed: i64 = values[1].parse()?;
            let (chosen, order, next) = if values[0] == "0" {
                let mut random = LegacyRandom::new(seed);
                let chosen = label(
                    pool.random_element(&mut |bound| random.next_int_bound(bound))?
                        .unwrap(),
                );
                let order = pool
                    .shuffled_elements(&mut |bound| random.next_int_bound(bound))?
                    .into_iter()
                    .map(label)
                    .collect::<String>();
                (chosen, order, random.next_long())
            } else {
                let mut random = DecorationRandom::new(seed);
                let chosen = label(
                    pool.random_element(&mut |bound| random.next_int_bound(bound))?
                        .unwrap(),
                );
                let order = pool
                    .shuffled_elements(&mut |bound| random.next_int_bound(bound))?
                    .into_iter()
                    .map(label)
                    .collect::<String>();
                (chosen, order, random.next_long())
            };
            assert_eq!(chosen.to_string(), values[2]);
            assert_eq!(order, values[3]);
            assert_eq!(next, values[4].parse::<i64>()?);
            count += 1;
        }
        assert_eq!(count, 12);
        Ok(())
    }
}
