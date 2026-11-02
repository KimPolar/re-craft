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

fn load_feature(provider: &mut impl ResourceProvider, id: &ResourceId) -> Result<Value> {
    let path = format!("data/{}/worldgen/feature/{}.json", id.namespace, id.path);
    let bytes = provider
        .read_data(&path)?
        .with_context(|| format!("missing feature {path}"))?;
    serde_json::from_slice(&bytes).with_context(|| format!("invalid feature {path}"))
}

fn parse_config<S: Clone + Eq>(
    value: &Value,
    expected: &str,
    resolve: &mut impl FnMut(&Value) -> Result<S>,
) -> Result<(i32, f32, Vec<Target<S>>)> {
    let kind = id(value, "type")?;
    ensure!(
        kind.namespace == "minecraft" && kind.path == expected,
        "wrong ore feature type"
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
    Ok((size, discard, targets))
}

fn choose_target<'a, S: Clone + Eq, W: OreWorld<State = S>>(
    targets: &'a [Target<S>],
    discard: f32,
    world: &mut W,
    state: &S,
    pos: [i32; 3],
    random: &mut DecorationRandom,
    mut neighbor: impl FnMut(&mut W, [i32; 3]) -> Result<S>,
) -> Result<Option<&'a S>> {
    for target in targets {
        if !target.rule.test(world, state, pos, random)? {
            continue;
        }
        let skip = if discard <= 0.0 {
            true
        } else if discard >= 1.0 {
            false
        } else {
            random.next_float() >= discard
        };
        if !skip {
            let mut adjacent = false;
            for offset in [
                [0, -1, 0],
                [0, 1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [-1, 0, 0],
                [1, 0, 0],
            ] {
                let p = [
                    pos[0].wrapping_add(offset[0]),
                    pos[1].wrapping_add(offset[1]),
                    pos[2].wrapping_add(offset[2]),
                ];
                let block = neighbor(world, p)?;
                if world.is_air(&block)? {
                    adjacent = true;
                    break;
                }
            }
            if adjacent {
                continue;
            }
        }
        return Ok(Some(&target.replacement));
    }
    Ok(None)
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
        let value = load_feature(provider, id)?;
        Self::decode(&value, resolve)
            .with_context(|| format!("in feature {}:{}", id.namespace, id.path))
    }
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let (size, discard, targets) = parse_config(value, "scattered_ore", resolve)?;
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
            if let Some(replacement) = choose_target(
                &self.targets,
                self.discard,
                world,
                &state,
                pos,
                random,
                |w, p| w.block(p),
            )? {
                let _ = world.set_block(pos, replacement, 2)?;
            }
        }
        Ok(true)
    }
}

/// The ordinary 26.3 ellipsoid ore vein used for coal, iron and other ores.
pub struct OreVein<S> {
    size: i32,
    discard: f32,
    targets: Vec<Target<S>>,
}
impl<S: Clone + Eq> OreVein<S> {
    pub fn load(
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        let value = load_feature(provider, id)?;
        Self::decode(&value, resolve)
            .with_context(|| format!("in feature {}:{}", id.namespace, id.path))
    }
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let (size, discard, targets) = parse_config(value, "ore", resolve)?;
        Ok(Self {
            size,
            discard,
            targets,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut result = BTreeSet::new();
        for target in &self.targets {
            target.rule.collect_tags(&mut result);
        }
        result
    }
    /// Applies the feature directly to generation sections. An error can leave
    /// partial changes and consumed randomness; discard the generation attempt.
    pub fn place(
        &self,
        world: &mut impl OreVeinWorld<State = S>,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        let angle = random.next_float() * std::f32::consts::PI;
        let length = self.size as f32 / 8.0;
        let padding = ((self.size as f32 / 16.0 * 2.0 + 1.0) / 2.0).ceil() as i32;
        let dx = f64::from(angle).sin() * f64::from(length);
        let dz = f64::from(angle).cos() * f64::from(length);
        let ends = [
            f64::from(origin[0]) + dx,
            f64::from(origin[0]) - dx,
            f64::from(origin[2]) + dz,
            f64::from(origin[2]) - dz,
            f64::from(
                origin[1]
                    .wrapping_add(random.next_int_bound(3).unwrap())
                    .wrapping_sub(2),
            ),
            f64::from(
                origin[1]
                    .wrapping_add(random.next_int_bound(3).unwrap())
                    .wrapping_sub(2),
            ),
        ];
        let reach = length.ceil() as i32;
        let min_x = origin[0]
            .checked_sub(reach + padding)
            .context("ore X bounds overflow")?;
        let min_y = origin[1]
            .checked_sub(2 + padding)
            .context("ore Y bounds overflow")?;
        let min_z = origin[2]
            .checked_sub(reach + padding)
            .context("ore Z bounds overflow")?;
        let width = 2 * (reach + padding);
        let height = 2 * (2 + padding);
        let max_x = min_x.checked_add(width).context("ore X bounds overflow")?;
        let max_z = min_z.checked_add(width).context("ore Z bounds overflow")?;
        let mut has_ground = false;
        'surface: for x in min_x..=max_x {
            for z in min_z..=max_z {
                if min_y <= world.ocean_floor_height(x, z)? {
                    has_ground = true;
                    break 'surface;
                }
            }
        }
        if !has_ground {
            return Ok(false);
        }
        let mut spheres = Vec::with_capacity(self.size as usize);
        for i in 0..self.size {
            let t = i as f32 / self.size as f32;
            let t64 = f64::from(t);
            let lerp = |a: f64, b: f64| a + t64 * (b - a);
            let scale = random.next_double() * f64::from(self.size) / 16.0;
            let wave = crate::carver::cave::sin(f64::from(std::f32::consts::PI * t)) + 1.0;
            spheres.push([
                lerp(ends[0], ends[1]),
                lerp(ends[4], ends[5]),
                lerp(ends[2], ends[3]),
                (f64::from(wave) * scale + 1.0) / 2.0,
            ]);
        }
        for i in 0..spheres.len().saturating_sub(1) {
            if spheres[i][3] <= 0.0 {
                continue;
            }
            for j in i + 1..spheres.len() {
                if spheres[j][3] <= 0.0 {
                    continue;
                }
                let dx = spheres[i][0] - spheres[j][0];
                let dy = spheres[i][1] - spheres[j][1];
                let dz = spheres[i][2] - spheres[j][2];
                let dr = spheres[i][3] - spheres[j][3];
                if dr * dr > dx * dx + dy * dy + dz * dz {
                    if dr > 0.0 {
                        spheres[j][3] = -1.0
                    } else {
                        spheres[i][3] = -1.0
                    }
                }
            }
        }
        let initial = (width as usize) * (height as usize) * (width as usize);
        let mut visited = vec![false; initial];
        let mut placed = 0usize;
        for [cx, cy, cz, radius] in spheres {
            if radius < 0.0 {
                continue;
            }
            let floor = |value: f64| value.floor() as i32;
            let x0 = floor(cx - radius).max(min_x);
            let y0 = floor(cy - radius).max(min_y);
            let z0 = floor(cz - radius).max(min_z);
            let x1 = floor(cx + radius).max(x0);
            let y1 = floor(cy + radius).max(y0);
            let z1 = floor(cz + radius).max(z0);
            for x in x0..=x1 {
                let nx = (f64::from(x) + 0.5 - cx) / radius;
                if nx * nx >= 1.0 {
                    continue;
                }
                for y in y0..=y1 {
                    let ny = (f64::from(y) + 0.5 - cy) / radius;
                    if nx * nx + ny * ny >= 1.0 {
                        continue;
                    }
                    for z in z0..=z1 {
                        let nz = (f64::from(z) + 0.5 - cz) / radius;
                        if nx * nx + ny * ny + nz * nz >= 1.0 || world.outside_build_height(y) {
                            continue;
                        }
                        let index = (x - min_x) as usize
                            + (y - min_y) as usize * width as usize
                            + (z - min_z) as usize * width as usize * height as usize;
                        if index >= visited.len() {
                            visited.resize(index + 1, false);
                        }
                        if visited[index] {
                            continue;
                        }
                        visited[index] = true;
                        let pos = [x, y, z];
                        if !world.ensure_can_write(pos)? {
                            continue;
                        }
                        let Some(state) = world.section_block(pos)? else {
                            continue;
                        };
                        if let Some(replacement) = choose_target(
                            &self.targets,
                            self.discard,
                            world,
                            &state,
                            pos,
                            random,
                            |w, p| w.section_neighbor(p),
                        )? {
                            world.set_section_block(pos, replacement)?;
                            placed += 1;
                        }
                    }
                }
            }
        }
        Ok(placed > 0)
    }
}

/// Java's regular ore feature reads and writes LevelChunkSection directly.
pub trait OreVeinWorld: OreWorld {
    /// OCEAN_FLOOR_WG first-available Y.
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32>;
    fn outside_build_height(&self, y: i32) -> bool;
    fn ensure_can_write(&mut self, pos: [i32; 3]) -> Result<bool>;
    /// None corresponds to BulkSectionAccess returning no section.
    fn section_block(&mut self, pos: [i32; 3]) -> Result<Option<Self::State>>;
    /// Neighbor reads use BulkSectionAccess, including its outside-height air.
    fn section_neighbor(&mut self, pos: [i32; 3]) -> Result<Self::State>;
    fn set_section_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<()>;
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
