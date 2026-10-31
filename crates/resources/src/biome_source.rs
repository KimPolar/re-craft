//! Multi-noise biome selection from the active world data packs.
//! The built-in overworld and End biome builders are not yet implemented.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::{
    density::DensityEvaluator,
    noise_router::NoiseRouter,
    packs::{ResourceId, ResourceProvider},
};

const DIMENSIONS: usize = 7;
const BRANCHING: usize = 19;
const FIELDS: [&str; 6] = [
    "temperature",
    "humidity",
    "continentalness",
    "erosion",
    "depth",
    "weirdness",
];

/// Java multiplies in float before converting to long, truncating toward zero.
pub fn quantize(value: f32) -> i64 {
    (value * 10_000.0_f32) as i64
}

#[derive(Clone, Copy, Debug)]
struct Interval {
    min: i64,
    max: i64,
}
impl Interval {
    fn distance(self, value: i64) -> i64 {
        let above = value.wrapping_sub(self.max);
        if above > 0 {
            above
        } else {
            self.min.wrapping_sub(value).max(0)
        }
    }
    fn center(self) -> i64 {
        self.min.wrapping_add(self.max) / 2
    }
}

#[derive(Clone)]
struct Node {
    space: [Interval; DIMENSIONS],
    kind: NodeKind,
}
#[derive(Clone)]
enum NodeKind {
    Leaf(usize),
    Branch(Vec<usize>),
}

struct ClimateTree {
    nodes: Vec<Node>,
    root: usize,
    biomes: Vec<ResourceId>,
}
impl ClimateTree {
    fn new(entries: Vec<([Interval; DIMENSIONS], ResourceId)>) -> Result<Self> {
        ensure!(
            !entries.is_empty() && entries.len() <= 100_000,
            "invalid biome parameter count"
        );
        let mut biomes = Vec::with_capacity(entries.len());
        let mut nodes = Vec::with_capacity(entries.len());
        for (space, biome) in entries {
            nodes.push(Node {
                space,
                kind: NodeKind::Leaf(biomes.len()),
            });
            biomes.push(biome);
        }
        let root = Self::build(&mut nodes, (0..biomes.len()).collect());
        // Building tries several bucket layouts. Retain only the chosen tree so
        // candidate nodes and their child vectors do not occupy runtime memory.
        fn compact(old: &[Node], id: usize, output: &mut Vec<Node>) -> usize {
            let kind = match &old[id].kind {
                NodeKind::Leaf(value) => NodeKind::Leaf(*value),
                NodeKind::Branch(children) => NodeKind::Branch(
                    children
                        .iter()
                        .map(|&child| compact(old, child, output))
                        .collect(),
                ),
            };
            output.push(Node {
                space: old[id].space,
                kind,
            });
            output.len() - 1
        }
        let mut compacted = Vec::with_capacity(biomes.len());
        let root = compact(&nodes, root, &mut compacted);
        Ok(Self {
            nodes: compacted,
            root,
            biomes,
        })
    }
    fn branch(nodes: &mut Vec<Node>, children: Vec<usize>) -> usize {
        let mut space = nodes[children[0]].space;
        for &child in &children[1..] {
            for (i, axis) in space.iter_mut().enumerate() {
                axis.min = axis.min.min(nodes[child].space[i].min);
                axis.max = axis.max.max(nodes[child].space[i].max);
            }
        }
        nodes.push(Node {
            space,
            kind: NodeKind::Branch(children),
        });
        nodes.len() - 1
    }
    fn sort(nodes: &[Node], children: &mut [usize], start: usize, absolute: bool) {
        children.sort_by(|a, b| {
            for offset in 0..DIMENSIONS {
                let axis = (start + offset) % DIMENSIONS;
                let x = nodes[*a].space[axis].center();
                let y = nodes[*b].space[axis].center();
                let order = if absolute {
                    x.wrapping_abs().cmp(&y.wrapping_abs())
                } else {
                    x.cmp(&y)
                };
                if !order.is_eq() {
                    return order;
                }
            }
            std::cmp::Ordering::Equal
        });
    }
    fn build(nodes: &mut Vec<Node>, mut children: Vec<usize>) -> usize {
        if children.len() == 1 {
            return children[0];
        }
        if children.len() <= BRANCHING {
            children.sort_by_key(|&i| {
                nodes[i]
                    .space
                    .iter()
                    .fold(0i64, |sum, p| sum.wrapping_add(p.center().wrapping_abs()))
            });
            return Self::branch(nodes, children);
        }
        let mut bucket_size = 1usize;
        while bucket_size * BRANCHING < children.len() {
            bucket_size *= BRANCHING;
        }
        let mut best_cost = i64::MAX;
        let mut best_axis = 0;
        let mut best = Vec::new();
        for axis in 0..DIMENSIONS {
            Self::sort(nodes, &mut children, axis, false);
            let buckets: Vec<_> = children
                .chunks(bucket_size)
                .map(|chunk| Self::branch(nodes, chunk.to_vec()))
                .collect();
            let cost = buckets.iter().fold(0i64, |sum, &id| {
                nodes[id].space.iter().fold(sum, |s, p| {
                    s.wrapping_add(p.max.wrapping_sub(p.min).wrapping_abs())
                })
            });
            if cost < best_cost {
                best_cost = cost;
                best_axis = axis;
                best = buckets;
            }
        }
        Self::sort(nodes, &mut best, best_axis, true);
        let mut rebuilt = Vec::with_capacity(best.len());
        for id in best {
            let NodeKind::Branch(children) = &nodes[id].kind else {
                unreachable!()
            };
            rebuilt.push(Self::build(nodes, children.clone()));
        }
        Self::branch(nodes, rebuilt)
    }
    fn distance(&self, id: usize, target: &[i64; DIMENSIONS]) -> i64 {
        self.nodes[id]
            .space
            .iter()
            .zip(target)
            .fold(0i64, |sum, (p, value)| {
                let d = p.distance(*value);
                sum.wrapping_add(d.wrapping_mul(d))
            })
    }
    fn search_node(
        &self,
        id: usize,
        target: &[i64; DIMENSIONS],
        previous: Option<usize>,
    ) -> Option<usize> {
        match &self.nodes[id].kind {
            NodeKind::Leaf(_) => Some(id),
            NodeKind::Branch(children) => {
                let mut best = previous;
                let mut best_distance = best.map_or(i64::MAX, |node| self.distance(node, target));
                for &child in children {
                    if self.distance(child, target) < best_distance {
                        if let Some(result) = self.search_node(child, target, best) {
                            let distance = self.distance(result, target);
                            if distance < best_distance {
                                best = Some(result);
                                best_distance = distance;
                            }
                        }
                    }
                }
                best
            }
        }
    }
}

/// A search object keeps the previous result like Java's thread-local tree cache.
pub struct BiomeSource {
    kind: SourceKind,
    previous: Option<usize>,
}
enum SourceKind {
    Fixed(ResourceId),
    Multi(ClimateTree),
}
impl BiomeSource {
    /// Parse the generator's active biome_source definition. Unknown presets fail.
    pub fn load(provider: &mut impl ResourceProvider, value: &Value) -> Result<Self> {
        let registry: BTreeSet<_> = provider.list_data("worldgen/biome")?.into_iter().collect();
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("biome source type missing")?,
        )?;
        ensure!(
            kind.namespace == "minecraft",
            "unsupported biome source namespace"
        );
        let source = match kind.path.as_str() {
            "fixed" => {
                let id = ResourceId::parse(
                    value
                        .get("biome")
                        .and_then(Value::as_str)
                        .context("fixed biome missing")?,
                )?;
                ensure!(
                    registry.contains(&id),
                    "fixed biome is absent from the active registry"
                );
                SourceKind::Fixed(id)
            }
            "multi_noise" => {
                let entries = if let Some(biomes) = value.get("biomes") {
                    parse_entries(biomes, &registry)?
                } else {
                    let name = value
                        .get("preset")
                        .and_then(Value::as_str)
                        .context("missing biome preset")?;
                    let id = ResourceId::parse(name)?;
                    let path = format!(
                        "data/{}/worldgen/multi_noise_biome_source_parameter_list/{}.json",
                        id.namespace, id.path
                    );
                    let bytes = provider
                        .read_data(&path)?
                        .with_context(|| format!("missing biome preset {name}"))?;
                    let preset: Value =
                        serde_json::from_slice(&bytes).context("invalid biome preset JSON")?;
                    let preset = ResourceId::parse(
                        preset
                            .get("preset")
                            .and_then(Value::as_str)
                            .context("invalid biome preset")?,
                    )?;
                    if preset.namespace == "minecraft" && preset.path == "nether" {
                        nether_entries(&registry)?
                    } else {
                        bail!(
                            "unsupported biome preset {}:{}",
                            preset.namespace,
                            preset.path
                        )
                    }
                };
                SourceKind::Multi(ClimateTree::new(entries)?)
            }
            _ => bail!("unsupported biome source {}:{}", kind.namespace, kind.path),
        };
        Ok(Self {
            kind: source,
            previous: None,
        })
    }
    /// Select from six single-precision climate components in Java's order.
    pub fn select(&mut self, climate: [f32; 6]) -> &ResourceId {
        match &self.kind {
            SourceKind::Fixed(biome) => biome,
            SourceKind::Multi(tree) => {
                let mut target = [0i64; DIMENSIONS];
                for (slot, value) in target.iter_mut().zip(climate) {
                    *slot = quantize(value);
                }
                let leaf = tree
                    .search_node(tree.root, &target, self.previous)
                    .expect("tree has at least one biome");
                self.previous = Some(leaf);
                let NodeKind::Leaf(index) = tree.nodes[leaf].kind else {
                    unreachable!()
                };
                &tree.biomes[index]
            }
        }
    }
    /// Sample the noise router at a noise-biome (quart) coordinate. Chunk-volume
    /// interpolation uses a separate Java path and is not implemented here.
    pub fn sample_quart(
        &mut self,
        router: &NoiseRouter,
        evaluator: &mut DensityEvaluator,
        quart: [i32; 3],
    ) -> &ResourceId {
        let [x, y, z] = quart.map(|v| v.wrapping_mul(4));
        let climate = router.sample_climate(evaluator, x, y, z);
        self.select(climate)
    }
}

fn parse_parameter(value: &Value) -> Result<Interval> {
    fn number(value: &Value) -> Result<f32> {
        let number = value
            .as_f64()
            .context("climate parameter must be a number")? as f32;
        ensure!(
            number.is_finite() && (-2.0..=2.0).contains(&number),
            "climate parameter out of range"
        );
        Ok(number)
    }
    let (min, max) = match value {
        Value::Number(_) => {
            let value = number(value)?;
            (value, value)
        }
        Value::Array(values) if values.len() == 2 => (number(&values[0])?, number(&values[1])?),
        Value::Object(values) => (
            number(values.get("min").context("missing min")?)?,
            number(values.get("max").context("missing max")?)?,
        ),
        _ => bail!("invalid climate parameter interval"),
    };
    ensure!(min <= max, "climate interval is reversed");
    Ok(Interval {
        min: quantize(min),
        max: quantize(max),
    })
}
fn parse_entries(
    value: &Value,
    registry: &BTreeSet<ResourceId>,
) -> Result<Vec<([Interval; DIMENSIONS], ResourceId)>> {
    let values = value.as_array().context("biomes must be an array")?;
    ensure!(
        !values.is_empty() && values.len() <= 100_000,
        "invalid biome parameter count"
    );
    values
        .iter()
        .map(|entry| {
            let biome = ResourceId::parse(
                entry
                    .get("biome")
                    .and_then(Value::as_str)
                    .context("biome ID missing")?,
            )?;
            ensure!(
                registry.contains(&biome),
                "biome is absent from the active registry"
            );
            let parameters = entry
                .get("parameters")
                .context("biome parameters missing")?;
            let mut intervals = Vec::with_capacity(DIMENSIONS);
            for field in FIELDS {
                intervals.push(parse_parameter(
                    parameters
                        .get(field)
                        .with_context(|| format!("missing climate field {field}"))?,
                )?);
            }
            let offset = parameters
                .get("offset")
                .and_then(Value::as_f64)
                .context("missing biome offset")? as f32;
            ensure!(
                offset.is_finite() && (0.0..=1.0).contains(&offset),
                "invalid biome offset"
            );
            let offset = quantize(offset);
            intervals.push(Interval {
                min: offset,
                max: offset,
            });
            Ok((intervals.try_into().unwrap(), biome))
        })
        .collect()
}
fn nether_entries(
    registry: &BTreeSet<ResourceId>,
) -> Result<Vec<([Interval; DIMENSIONS], ResourceId)>> {
    let definitions = [
        ("nether_wastes", [0.0, 0.0, 0.0]),
        ("soul_sand_valley", [0.0, -0.5, 0.0]),
        ("crimson_forest", [0.4, 0.0, 0.0]),
        ("warped_forest", [0.0, 0.5, 0.375]),
        ("basalt_deltas", [-0.5, 0.0, 0.175]),
    ];
    definitions
        .into_iter()
        .map(|(name, [temperature, humidity, offset])| {
            let id = ResourceId::parse(name)?;
            ensure!(registry.contains(&id), "nether biome {name} is missing");
            let point = |x| {
                let q = quantize(x);
                Interval { min: q, max: q }
            };
            Ok((
                [
                    point(temperature),
                    point(humidity),
                    point(0.0),
                    point(0.0),
                    point(0.0),
                    point(0.0),
                    point(offset),
                ],
                id,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
            Ok(vec![
                ResourceId::parse("example:a")?,
                ResourceId::parse("example:b")?,
            ])
        }
    }
    #[test]
    fn checks_registry_intervals_and_history_for_ties() {
        let mut data = Data;
        let point = |biome, temperature| {
            json!({"biome":biome,"parameters":{
                "temperature":temperature,"humidity":0,"continentalness":0,"erosion":0,"depth":0,"weirdness":0,"offset":0
            }})
        };
        let definition = json!({"type":"multi_noise","biomes":[point("example:a",json!(-1)),point("example:b",json!(1))]});
        let mut source = BiomeSource::load(&mut data, &definition).unwrap();
        assert_eq!(source.select([1.0, 0.0, 0.0, 0.0, 0.0, 0.0]).path, "b");
        assert_eq!(source.select([0.0; 6]).path, "b");
        assert_eq!(source.select([-1.0, 0.0, 0.0, 0.0, 0.0, 0.0]).path, "a");
        assert_eq!(source.select([0.0; 6]).path, "a");
        let settings = json!({"noise":{"min_y":-16,"height":32},"legacy_random_source":false,
            "noise_router":{"final_density":0,"temperature":{"type":"gradient","axis":"y",
                "from_coordinate":-16,"to_coordinate":16,"from_value":-1,"to_value":1},
                "vegetation":0,"continents":0,"erosion":0,"depth":0,"ridges":0,"chunk_surface_level":0}});
        let router = NoiseRouter::from_json_without_terrain_adjustments(
            &mut data,
            &serde_json::to_vec(&settings).unwrap(),
            42,
        )
        .unwrap();
        let mut evaluator = DensityEvaluator::default();
        assert_eq!(
            source
                .sample_quart(&router, &mut evaluator, [0, -4, 0])
                .path,
            "a"
        );
        assert_eq!(
            source.sample_quart(&router, &mut evaluator, [0, 4, 0]).path,
            "b"
        );
        assert!(BiomeSource::load(&mut data, &json!({"type":"fixed","biome":"missing"})).is_err());
        assert!(
            BiomeSource::load(
                &mut data,
                &json!({"type":"multi_noise","biomes":[point("example:a",json!([1,-1]))]})
            )
            .is_err()
        );
        assert!(BiomeSource::load(&mut data, &json!({"type":"multi_noise","biomes":[]})).is_err());
    }
}
