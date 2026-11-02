//! Data-driven ore replacement rules and Java 26.3 scattered ore placement.
//! The world supplies authoritative block states, active tags and setBlock effects.
use crate::packs::{ResourceId, ResourceProvider};
use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
enum Rule<S> {
    Always,
    Block(ResourceId),
    State(S),
    Tag(ResourceId),
    Height(i32, i32),
    RandomBlock(ResourceId, f32),
    RandomState(S, f32),
    All(Vec<Rule<S>>),
    Any(Vec<Rule<S>>),
    Not(Box<Rule<S>>),
}
fn id(value: &Value, key: &str) -> Result<ResourceId> {
    ResourceId::parse(
        value
            .get(key)
            .and_then(Value::as_str)
            .with_context(|| format!("missing {key}"))?,
    )
}
fn integer(value: &Value, key: &str) -> Result<i32> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|x| i32::try_from(x).ok())
        .with_context(|| format!("invalid {key}"))
}
fn probability(value: &Value, key: &str) -> Result<f32> {
    let p = value
        .get(key)
        .and_then(Value::as_f64)
        .context("missing probability")? as f32;
    ensure!(p.is_finite(), "invalid probability");
    Ok(p)
}
impl<S: Clone + Eq> Rule<S> {
    fn collect_tags(&self, out: &mut BTreeSet<ResourceId>) {
        match self {
            Self::Tag(id) => {
                out.insert(id.clone());
            }
            Self::All(rules) | Self::Any(rules) => {
                for rule in rules {
                    rule.collect_tags(out)
                }
            }
            Self::Not(rule) => rule.collect_tags(out),
            _ => {}
        }
    }
    fn parse(
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
        depth: u8,
    ) -> Result<Self> {
        ensure!(depth < 32, "ore rule nesting is too deep");
        let kind = id(value, "predicate_type")?;
        ensure!(
            kind.namespace == "minecraft",
            "unsupported ore rule namespace"
        );
        let rules = |resolve: &mut _, depth| -> Result<Vec<Self>> {
            let list = value
                .get("rules")
                .and_then(Value::as_array)
                .context("missing ore rules")?;
            ensure!(list.len() <= 4096, "too many ore rules");
            list.iter()
                .map(|v| Self::parse(v, resolve, depth + 1))
                .collect()
        };
        Ok(match kind.path.as_str() {
            "always_true" => Self::Always,
            "block_match" => Self::Block(id(value, "block")?),
            "blockstate_match" => Self::State(resolve(
                value.get("block_state").context("missing block state")?,
            )?),
            "tag_match" => Self::Tag(id(value, "tag")?),
            "height_match" => Self::Height(
                integer(value, "min_inclusive")?,
                integer(value, "max_inclusive")?,
            ),
            "random_block_match" => {
                Self::RandomBlock(id(value, "block")?, probability(value, "probability")?)
            }
            "random_blockstate_match" => Self::RandomState(
                resolve(value.get("block_state").context("missing block state")?)?,
                probability(value, "probability")?,
            ),
            "all_of" => Self::All(rules(resolve, depth)?),
            "any_of" => Self::Any(rules(resolve, depth)?),
            "not" => Self::Not(Box::new(Self::parse(
                value.get("rule").context("missing negated rule")?,
                resolve,
                depth + 1,
            )?)),
            name => bail!("unsupported ore rule {name}"),
        })
    }
    fn test<W: OreWorld<State = S>>(
        &self,
        world: &mut W,
        state: &S,
        pos: [i32; 3],
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        Ok(match self {
            Self::Always => true,
            Self::Block(id) => world.block_is(state, id)?,
            Self::State(expected) => state == expected,
            Self::Tag(tag) => world.in_tag(state, tag)?,
            Self::Height(min, max) => pos[1] >= *min && pos[1] <= *max,
            Self::RandomBlock(id, p) => world.block_is(state, id)? && random.next_float() < *p,
            Self::RandomState(expected, p) => state == expected && random.next_float() < *p,
            Self::All(rules) => {
                let mut yes = true;
                for rule in rules {
                    if !rule.test(world, state, pos, random)? {
                        yes = false;
                        break;
                    }
                }
                yes
            }
            Self::Any(rules) => {
                let mut yes = false;
                for rule in rules {
                    if rule.test(world, state, pos, random)? {
                        yes = true;
                        break;
                    }
                }
                yes
            }
            Self::Not(rule) => !rule.test(world, state, pos, random)?,
        })
    }
}

struct Target<S> {
    rule: Rule<S>,
    replacement: S,
}
pub struct ScatteredOre<S> {
    size: i32,
    discard: f32,
    targets: Vec<Target<S>>,
}
impl<S: Clone + Eq> ScatteredOre<S> {
    /// Tags referenced anywhere in the configured replacement predicates.
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut result = BTreeSet::new();
        for target in &self.targets {
            target.rule.collect_tags(&mut result);
        }
        result
    }
    pub fn load(
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        let path = format!("data/{}/worldgen/feature/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing feature {path}"))?;
        let value: Value =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid feature {path}"))?;
        Self::decode(&value, resolve).with_context(|| format!("in feature {path}"))
    }
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let kind = id(value, "type")?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "scattered_ore",
            "not a scattered ore feature"
        );
        let size = integer(value, "size")?;
        ensure!((0..=64).contains(&size), "invalid ore size");
        let discard = probability(value, "discard_chance_on_air_exposure")?;
        ensure!(
            (0.0..=1.0).contains(&discard),
            "invalid air exposure chance"
        );
        let entries = value
            .get("targets")
            .and_then(Value::as_array)
            .context("missing ore targets")?;
        ensure!(entries.len() <= 4096, "too many ore targets");
        let mut targets = Vec::with_capacity(entries.len());
        for target in entries {
            targets.push(Target {
                rule: Rule::parse(
                    target.get("target").context("missing ore rule")?,
                    resolve,
                    0,
                )?,
                replacement: resolve(target.get("state").context("missing replacement state")?)?,
            });
        }
        Ok(Self {
            size,
            discard,
            targets,
        })
    }
    /// Returns Java's feature result (always true, even if no blocks change).
    /// A failed world query can leave earlier writes applied: discard that attempt.
    pub fn place(
        &self,
        world: &mut impl OreWorld<State = S>,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        let count = random.next_int_bound(self.size + 1).unwrap();
        for i in 0..count {
            let spread = i.min(7) as f32;
            let mut offset = || {
                ((f64::from((random.next_float() - random.next_float()) * spread) + 0.5).floor())
                    as i32
            };
            let pos = [
                origin[0].wrapping_add(offset()),
                origin[1].wrapping_add(offset()),
                origin[2].wrapping_add(offset()),
            ];
            let state = world.block(pos)?;
            for target in &self.targets {
                if !target.rule.test(world, &state, pos, random)? {
                    continue;
                }
                let skip_air = if self.discard <= 0.0 {
                    true
                } else if self.discard >= 1.0 {
                    false
                } else {
                    random.next_float() >= self.discard
                };
                if !skip_air {
                    let mut adjacent = false;
                    for offset in [
                        [0, -1, 0],
                        [0, 1, 0],
                        [0, 0, -1],
                        [0, 0, 1],
                        [-1, 0, 0],
                        [1, 0, 0],
                    ] {
                        let neighbour = [
                            pos[0].wrapping_add(offset[0]),
                            pos[1].wrapping_add(offset[1]),
                            pos[2].wrapping_add(offset[2]),
                        ];
                        let block = world.block(neighbour)?;
                        if world.is_air(&block)? {
                            adjacent = true;
                            break;
                        }
                    }
                    if adjacent {
                        continue;
                    }
                }
                let _ = world.set_block(pos, &target.replacement, 2)?;
                break;
            }
        }
        Ok(true)
    }
}

/// Tag resolution against the complete active block registry and data-pack stack.
pub struct OreBlockTags {
    registry: BTreeSet<String>,
    tags: BTreeMap<ResourceId, BTreeSet<String>>,
}
impl OreBlockTags {
    pub fn load(
        provider: &mut impl ResourceProvider,
        registry: &BTreeSet<String>,
        tags: &BTreeSet<ResourceId>,
    ) -> Result<Self> {
        ensure!(!registry.is_empty(), "block registry is empty");
        for name in registry {
            let id = ResourceId::parse(name)?;
            ensure!(
                *name == format!("{}:{}", id.namespace, id.path),
                "noncanonical block registry ID"
            );
        }
        let mut raw = BTreeMap::new();
        let mut failed = BTreeSet::new();
        for id in tags {
            crate::transforms::read_block_tag(provider, id, &mut raw, &mut failed, 0)?;
        }
        let mut cache = BTreeMap::new();
        let mut values = BTreeMap::new();
        for id in tags {
            values.insert(
                id.clone(),
                crate::heightmap_tags::resolve(
                    id,
                    &raw,
                    registry,
                    &mut cache,
                    &mut BTreeSet::new(),
                )
                .with_context(|| format!("cannot resolve ore tag {}:{}", id.namespace, id.path))?,
            );
        }
        Ok(Self {
            registry: registry.clone(),
            tags: values,
        })
    }
    pub fn contains(&self, tag: &ResourceId, block: &str) -> Result<bool> {
        ensure!(
            self.registry.contains(block),
            "block {block} is absent from registry"
        );
        Ok(self
            .tags
            .get(tag)
            .context("ore tag not loaded")?
            .contains(block))
    }
}

/// The caller must resolve states and block IDs against the same active game
/// registry and data packs. State equality must match Java's canonical states;
/// unknown blocks or tags must be reported as errors rather than non-matches.
pub trait OreWorld {
    type State: Clone + Eq;
    fn block(&mut self, pos: [i32; 3]) -> Result<Self::State>;
    fn block_is(&mut self, state: &Self::State, id: &ResourceId) -> Result<bool>;
    fn in_tag(&mut self, state: &Self::State, tag: &ResourceId) -> Result<bool>;
    fn is_air(&mut self, state: &Self::State) -> Result<bool>;
    /// Apply real WorldGenLevel.setBlock effects, or return an error if unsupported.
    fn set_block(&mut self, pos: [i32; 3], state: &Self::State, flags: i32) -> Result<bool>;
}
