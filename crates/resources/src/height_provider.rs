//! Java 26.3 vertical anchors and world-generation height providers.
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::packs::ResourceId;

#[derive(Clone, Copy, Debug)]
pub struct HeightContext {
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
}

#[derive(Clone, Copy, Debug)]
enum Anchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
    RelativeToSeaLevel(i32),
}

fn int(value: &Value) -> Result<i32> {
    value
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .context("invalid height integer")
}

impl Anchor {
    fn decode(value: &Value) -> Result<Self> {
        let obj = value
            .as_object()
            .context("vertical anchor must be an object")?;
        let mut found = None;
        for name in [
            "absolute",
            "above_bottom",
            "below_top",
            "relative_to_sea_level",
        ] {
            if let Some(value) = obj.get(name) {
                let n = int(value)?;
                ensure!((-2032..=2031).contains(&n), "height anchor out of range");
                ensure!(found.is_none(), "ambiguous vertical anchor");
                found = Some(match name {
                    "absolute" => Self::Absolute(n),
                    "above_bottom" => Self::AboveBottom(n),
                    "below_top" => Self::BelowTop(n),
                    _ => Self::RelativeToSeaLevel(n),
                });
            }
        }
        found.context("missing vertical anchor")
    }
    fn resolve(self, context: HeightContext) -> i32 {
        match self {
            Self::Absolute(n) => n,
            Self::AboveBottom(n) => context.min_y.wrapping_add(n),
            Self::BelowTop(n) => context
                .min_y
                .wrapping_add(context.height)
                .wrapping_sub(1)
                .wrapping_sub(n),
            Self::RelativeToSeaLevel(n) => context.sea_level.wrapping_add(n),
        }
    }
}

#[derive(Clone, Debug)]
enum Provider {
    Constant(Anchor),
    Uniform(Anchor, Anchor),
    Trapezoid(Anchor, Anchor, i32),
    Biased(Anchor, Anchor, i32),
    VeryBiased(Anchor, Anchor, i32),
    Weighted(Vec<(i32, Provider)>, i32),
}

/// An immutable decoded provider; sampling uses the caller's structure RNG.
#[derive(Clone, Debug)]
pub struct HeightProvider(Provider);

impl HeightProvider {
    pub fn decode(value: &Value) -> Result<Self> {
        Ok(Self(Provider::decode(value, 0)?))
    }
    pub fn sample(
        &self,
        context: HeightContext,
        random: &mut impl FnMut(i32) -> Option<i32>,
    ) -> Result<i32> {
        self.0.sample(context, random)
    }
}

fn draw(random: &mut impl FnMut(i32) -> Option<i32>, bound: i32) -> Result<i32> {
    ensure!(bound > 0, "invalid height random bound");
    let value = random(bound).context("height random source rejected bound")?;
    ensure!(
        (0..bound).contains(&value),
        "height random result out of bounds"
    );
    Ok(value)
}
fn between(random: &mut impl FnMut(i32) -> Option<i32>, min: i32, max: i32) -> Result<i32> {
    let bound = max.wrapping_sub(min).wrapping_add(1);
    Ok(min.wrapping_add(draw(random, bound)?))
}
fn next(random: &mut impl FnMut(i32) -> Option<i32>, min: i32, max: i32) -> Result<i32> {
    if min >= max {
        Ok(min)
    } else {
        between(random, min, max)
    }
}

impl Provider {
    fn decode(value: &Value, depth: usize) -> Result<Self> {
        ensure!(depth <= 64, "height provider nesting too deep");
        // HeightProvider.CODEC tries the anchor codec before dispatch, so an
        // otherwise valid anchor can include an unrelated `type` field.
        if let Ok(anchor) = Anchor::decode(value) {
            return Ok(Self::Constant(anchor));
        }
        let id = ResourceId::parse(
            value["type"]
                .as_str()
                .context("invalid height provider type")?,
        )?;
        ensure!(
            id.namespace == "minecraft",
            "unsupported height provider namespace"
        );
        let field = |name| {
            value
                .get(name)
                .with_context(|| format!("missing height provider {name}"))
        };
        let min = || Anchor::decode(field("min_inclusive")?);
        let max = || Anchor::decode(field("max_inclusive")?);
        let optional_int = |name, default| {
            value
                .get(name)
                .map(int)
                .transpose()
                .map(|n| n.unwrap_or(default))
        };
        Ok(match id.path.as_str() {
            "constant" => Self::Constant(Anchor::decode(field("value")?)?),
            "uniform" => Self::Uniform(min()?, max()?),
            "trapezoid" => Self::Trapezoid(min()?, max()?, optional_int("plateau", 0)?),
            "biased_to_bottom" | "very_biased_to_bottom" => {
                let inner = optional_int("inner", 1)?;
                ensure!(inner >= 1, "invalid height provider inner range");
                if id.path == "biased_to_bottom" {
                    Self::Biased(min()?, max()?, inner)
                } else {
                    Self::VeryBiased(min()?, max()?, inner)
                }
            }
            "weighted_list" => {
                let entries = field("distribution")?
                    .as_array()
                    .context("invalid height distribution")?;
                ensure!(!entries.is_empty(), "empty height distribution");
                let mut total = 0_i32;
                let mut result = Vec::with_capacity(entries.len());
                for entry in entries {
                    let weight = int(entry.get("weight").context("missing height weight")?)?;
                    ensure!(weight >= 0, "negative height weight");
                    total = total
                        .checked_add(weight)
                        .context("height weight overflow")?;
                    result.push((
                        weight,
                        Self::decode(
                            entry.get("data").context("missing weighted height")?,
                            depth + 1,
                        )?,
                    ));
                }
                Self::Weighted(result, total)
            }
            _ => bail!("unsupported height provider {}", id.path),
        })
    }

    fn sample(
        &self,
        context: HeightContext,
        random: &mut impl FnMut(i32) -> Option<i32>,
    ) -> Result<i32> {
        match self {
            Self::Constant(anchor) => Ok(anchor.resolve(context)),
            Self::Uniform(a, b) => {
                let a = a.resolve(context);
                let b = b.resolve(context);
                if a > b { Ok(a) } else { between(random, a, b) }
            }
            Self::Trapezoid(a, b, plateau) => {
                let a = a.resolve(context);
                let b = b.resolve(context);
                if a > b {
                    return Ok(a);
                }
                let width = b.wrapping_sub(a);
                if *plateau >= width {
                    return between(random, a, b);
                }
                let slope = width.wrapping_sub(*plateau) / 2;
                let high = width.wrapping_sub(slope);
                Ok(a.wrapping_add(between(random, 0, high)?)
                    .wrapping_add(between(random, 0, slope)?))
            }
            Self::Biased(a, b, inner) => {
                let a = a.resolve(context);
                let b = b.resolve(context);
                let width = b.wrapping_sub(a).wrapping_sub(*inner).wrapping_add(1);
                if width <= 0 {
                    return Ok(a);
                }
                let first = draw(random, width)?;
                Ok(a.wrapping_add(draw(random, first.wrapping_add(*inner))?))
            }
            Self::VeryBiased(a, b, inner) => {
                let a = a.resolve(context);
                let b = b.resolve(context);
                if b.wrapping_sub(a).wrapping_sub(*inner).wrapping_add(1) <= 0 {
                    return Ok(a);
                }
                let first = next(random, a.wrapping_add(*inner), b)?;
                let second = next(random, a, first.wrapping_sub(1))?;
                next(random, a, second.wrapping_sub(1).wrapping_add(*inner))
            }
            Self::Weighted(entries, total) => {
                let mut choice = draw(random, *total)?;
                for (weight, provider) in entries {
                    choice -= weight;
                    if choice < 0 {
                        return provider.sample(context, random);
                    }
                }
                unreachable!("validated height weights")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::random::LegacyRandom;
    use serde_json::json;

    #[test]
    fn empty_ranges_and_singleton_sampling_follow_java_draws() -> Result<()> {
        let context = HeightContext {
            min_y: -64,
            height: 384,
            sea_level: 63,
        };
        let singleton = HeightProvider::decode(
            &json!({"type":"minecraft:uniform","min_inclusive":{"absolute":4},"max_inclusive":{"absolute":4}}),
        )?;
        let empty = HeightProvider::decode(
            &json!({"type":"minecraft:uniform","min_inclusive":{"absolute":4},"max_inclusive":{"absolute":3}}),
        )?;
        let mut calls = Vec::new();
        assert_eq!(
            singleton.sample(context, &mut |n| {
                calls.push(n);
                Some(0)
            })?,
            4
        );
        assert_eq!(
            empty.sample(context, &mut |n| {
                calls.push(n);
                Some(0)
            })?,
            4
        );
        assert_eq!(calls, [1]);
        assert!(HeightProvider::decode(&json!({"absolute":0,"above_bottom":0})).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn height_distributions_and_random_state_match_java() -> Result<()> {
        use std::{collections::BTreeMap, path::PathBuf, process::Command};
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
        let source = temp.path().join("HeightProviderOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/HeightProviderOracle.java"),
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
            .arg("HeightProviderOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let mut cases = BTreeMap::new();
        let mut seen = 0;
        let mut bounds_seen = false;
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["B", min, max, size] => {
                    assert_eq!((*min, *max, *size), ("-2032", "2031", "4064"));
                    bounds_seen = true;
                }
                ["D", id, json] => {
                    cases.insert(
                        id.parse::<usize>()?,
                        HeightProvider::decode(&serde_json::from_str::<Value>(json)?)?,
                    );
                }
                ["S", id, min, height, sea, seed, expected, after] => {
                    let provider = cases.get(&id.parse()?).context("missing height case")?;
                    let context = HeightContext {
                        min_y: min.parse()?,
                        height: height.parse()?,
                        sea_level: sea.parse()?,
                    };
                    let mut random = LegacyRandom::new(seed.parse()?);
                    assert_eq!(
                        provider.sample(context, &mut |bound| random.next_int_bound(bound))?,
                        expected.parse::<i32>()?,
                        "case {id}, seed {seed}"
                    );
                    assert_eq!(
                        random.next_long(),
                        after.parse::<i64>()?,
                        "RNG case {id}, seed {seed}"
                    );
                    seen += 1;
                }
                _ => {}
            }
        }
        assert!(bounds_seen);
        assert_eq!(seen, 15 * 3 * 6);
        Ok(())
    }
}
