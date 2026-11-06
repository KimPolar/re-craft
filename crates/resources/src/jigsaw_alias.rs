//! Resolve Java 26.3 jigsaw pool aliases using the structure's positional RNG.
use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use native_core::{
    normal_noise::{NoiseSeedFactory, NoiseSeedSource},
    random::LegacyRandom,
};
use serde_json::Value;

use crate::packs::ResourceId;

#[derive(Clone, Debug)]
struct Weighted<T> {
    values: Vec<(i32, T)>,
    total: i32,
}

impl<T> Weighted<T> {
    fn decode(value: &Value, mut parse: impl FnMut(&Value) -> Result<T>) -> Result<Self> {
        let values = value.as_array().context("expected weighted alias list")?;
        ensure!(!values.is_empty(), "empty weighted alias list");
        let mut decoded = Vec::with_capacity(values.len());
        let mut total = 0_i32;
        for value in values {
            let weight = value
                .get("weight")
                .and_then(Value::as_i64)
                .context("missing alias weight")?;
            ensure!(
                (0..=i64::from(i32::MAX)).contains(&weight),
                "invalid alias weight"
            );
            total = total
                .checked_add(weight as i32)
                .context("alias weight overflow")?;
            decoded.push((
                weight as i32,
                parse(value.get("data").context("missing weighted alias data")?)?,
            ));
        }
        Ok(Self {
            values: decoded,
            total,
        })
    }

    fn choose(&self, random: &mut LegacyRandom) -> Result<&T> {
        let mut selected = random
            .next_int_bound(self.total)
            .context("weighted alias has no positive weight")?;
        for (weight, value) in &self.values {
            selected -= weight;
            if selected < 0 {
                return Ok(value);
            }
        }
        unreachable!("validated alias weights")
    }
}

#[derive(Clone, Debug)]
enum Binding {
    Direct {
        alias: ResourceId,
        target: ResourceId,
    },
    Random {
        alias: ResourceId,
        targets: Weighted<ResourceId>,
    },
    Group(Weighted<Vec<Binding>>),
}

impl Binding {
    fn decode(value: &Value, depth: usize) -> Result<Self> {
        ensure!(depth <= 64, "alias group nesting is too deep");
        let id =
            |value: &Value| ResourceId::parse(value.as_str().context("invalid pool identifier")?);
        let field = |name| {
            value
                .get(name)
                .with_context(|| format!("missing alias {name}"))
        };
        let kind = id(field("type")?)?;
        ensure!(kind.namespace == "minecraft", "unsupported alias namespace");
        Ok(match kind.path.as_str() {
            "direct" => Self::Direct {
                alias: id(field("alias")?)?,
                target: id(field("target")?)?,
            },
            "random" => Self::Random {
                alias: id(field("alias")?)?,
                targets: Weighted::decode(field("targets")?, id)?,
            },
            "random_group" => Self::Group(Weighted::decode(field("groups")?, |value| {
                value
                    .as_array()
                    .context("alias group must be a list")?
                    .iter()
                    .map(|entry| Self::decode(entry, depth + 1))
                    .collect()
            })?),
            _ => bail!("unsupported pool alias type {}", kind.path),
        })
    }

    fn resolve(
        &self,
        random: &mut LegacyRandom,
        result: &mut BTreeMap<ResourceId, ResourceId>,
    ) -> Result<()> {
        match self {
            Self::Direct { alias, target } => insert(result, alias, target),
            Self::Random { alias, targets } => insert(result, alias, targets.choose(random)?),
            Self::Group(groups) => {
                for binding in groups.choose(random)? {
                    binding.resolve(random, result)?;
                }
                Ok(())
            }
        }
    }
}

fn insert(
    result: &mut BTreeMap<ResourceId, ResourceId>,
    alias: &ResourceId,
    target: &ResourceId,
) -> Result<()> {
    ensure!(
        !result.contains_key(alias),
        "duplicate pool alias {}:{}",
        alias.namespace,
        alias.path
    );
    result.insert(alias.clone(), target.clone());
    Ok(())
}

/// Parsed bindings retain their original order, including nested random groups.
#[derive(Clone, Debug)]
pub struct PoolAliases(Vec<Binding>);

impl PoolAliases {
    pub fn decode(value: &Value) -> Result<Self> {
        Ok(Self(
            value
                .as_array()
                .context("pool aliases must be a list")?
                .iter()
                .map(|value| Binding::decode(value, 0))
                .collect::<Result<_>>()?,
        ))
    }

    /// Alias selection uses a separate legacy positional RNG, not the
    /// structure-start RNG; the position includes the sampled start height.
    pub fn resolve(
        &self,
        world_seed: i64,
        position: [i32; 3],
    ) -> Result<BTreeMap<ResourceId, ResourceId>> {
        if self.0.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut random = LegacyRandom::new(world_seed).fork_noise_factory().at(
            position[0],
            position[1],
            position[2],
        );
        self.resolve_with_random(&mut random)
    }

    fn resolve_with_random(
        &self,
        random: &mut LegacyRandom,
    ) -> Result<BTreeMap<ResourceId, ResourceId>> {
        let mut result = BTreeMap::new();
        for binding in &self.0 {
            binding.resolve(random, &mut result)?;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn duplicate_and_invalid_aliases_fail_without_partial_output() -> Result<()> {
        let aliases = PoolAliases::decode(&json!([
            {"type":"minecraft:direct","alias":"example:a","target":"example:b"},
            {"type":"minecraft:direct","alias":"example:a","target":"example:c"}
        ]))?;
        assert!(aliases.resolve(0, [0; 3]).is_err());
        assert!(
            PoolAliases::decode(&json!([{"type":"minecraft:random","alias":"a","targets":[]}]))
                .is_err()
        );
        let aliases = PoolAliases::decode(
            &json!([{"type":"minecraft:random","alias":"a","targets":[{"data":"b","weight":0}]}]),
        )?;
        assert!(aliases.resolve(0, [0; 3]).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn trial_and_nested_aliases_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let _verified = crate::packs::ResourceStack::open(&game)?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
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
        let source = temp.path().join("JigsawAliasOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawAliasOracle.java"),
        )?;
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
            .arg("JigsawAliasOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let mut cases = BTreeMap::new();
        let mut checked = [0, 0];
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["D", mode, json] => {
                    cases.insert(
                        mode.parse::<usize>()?,
                        PoolAliases::decode(&serde_json::from_str::<Value>(json)?)?,
                    );
                }
                ["S", mode, seed, x, y, z, expected] => {
                    let case = cases.get(&mode.parse()?).context("missing case")?;
                    let seed = seed.parse()?;
                    let position = [x.parse()?, y.parse()?, z.parse()?];
                    let mut random = LegacyRandom::new(seed).fork_noise_factory().at(
                        position[0],
                        position[1],
                        position[2],
                    );
                    let map = case.resolve_with_random(&mut random)?;
                    assert_eq!(map, case.resolve(seed, position)?);
                    assert_eq!(
                        random.next_long(),
                        expected.parse::<i64>()?,
                        "mode {mode} seed {seed} position {position:?}"
                    );
                    checked[0] += 1;
                }
                ["P", mode, seed, x, y, z, alias, target] => {
                    let case = cases.get(&mode.parse()?).context("missing case")?;
                    let alias = ResourceId::parse(alias)?;
                    let map = case.resolve(seed.parse()?, [x.parse()?, y.parse()?, z.parse()?])?;
                    assert_eq!(
                        map.get(&alias).unwrap_or(&alias),
                        &ResourceId::parse(target)?
                    );
                    checked[1] += 1;
                }
                _ => {}
            }
        }
        assert_eq!(checked, [48, 192]);
        Ok(())
    }
}
