//! Data-driven 26.3 material rules. Evaluation requires a real surface context;
//! this module does not create biomes, noise, ore veins or terrain by itself.
use crate::packs::{ResourceId, ResourceProvider};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 100_000;

/// Inputs for one block, calculated by the chunk's material system.
pub struct SurfacePosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    pub stone_above: i32,
    pub stone_below: i32,
    pub surface_depth: i32,
    pub surface_secondary: f64,
    pub water_height: i32,
    pub min_surface_level: i32,
    pub gradient_x: i32,
    pub gradient_z: i32,
}

/// Data which cannot be inferred from a rule definition alone.
pub trait SurfaceInputs {
    fn biome_matches(&mut self, selector: &Value, position: &SurfacePosition) -> Result<bool>;
    fn cold_enough_to_snow(&mut self, position: &SurfacePosition) -> Result<bool>;
    fn noise(&mut self, name: &ResourceId, is_3d: bool, position: &SurfacePosition) -> Result<f64>;
    fn positional_random(&mut self, name: &ResourceId, position: &SurfacePosition) -> Result<f32>;
    /// Bandlands and ore veins must be supplied by their actual implementations.
    fn special_rule(
        &mut self,
        kind: &str,
        definition: &Value,
        position: &SurfacePosition,
    ) -> Result<Option<Arc<Value>>>;
}

#[derive(Clone, Copy)]
enum Anchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
    Sea(i32),
}
impl Anchor {
    fn parse(value: &Value) -> Result<Self> {
        let obj = value
            .as_object()
            .context("vertical anchor must be an object")?;
        for (name, constructor) in [
            ("absolute", Self::Absolute as fn(i32) -> Self),
            ("above_bottom", Self::AboveBottom),
            ("below_top", Self::BelowTop),
            ("relative_to_sea_level", Self::Sea),
        ] {
            if obj.contains_key(name) {
                return Ok(constructor(integer(value, name)?));
            }
        }
        bail!("unknown vertical anchor")
    }
    fn resolve(self, p: &SurfacePosition) -> i32 {
        match self {
            Self::Absolute(y) => y,
            Self::AboveBottom(n) => p.min_y.wrapping_add(n),
            Self::BelowTop(n) => p
                .min_y
                .wrapping_add(p.height)
                .wrapping_sub(1)
                .wrapping_sub(n),
            Self::Sea(n) => p.sea_level.wrapping_add(n),
        }
    }
}

enum Condition {
    Biome(Value),
    Temperature,
    AboveSurface,
    Hole,
    Steep,
    Noise {
        name: ResourceId,
        min: f64,
        max: f64,
        is_3d: bool,
    },
    Gradient {
        name: ResourceId,
        low: Anchor,
        high: Anchor,
    },
    Y {
        anchor: Anchor,
        multiplier: i32,
        add_stone: bool,
    },
    Water {
        offset: i32,
        multiplier: i32,
        add_stone: bool,
    },
    Stone {
        offset: i32,
        add_surface: bool,
        secondary: i32,
        ceiling: bool,
    },
    Not(Arc<Condition>),
}
impl Condition {
    fn test(&self, p: &SurfacePosition, inputs: &mut impl SurfaceInputs) -> Result<bool> {
        Ok(match self {
            Self::Biome(selector) => inputs.biome_matches(selector, p)?,
            Self::Temperature => inputs.cold_enough_to_snow(p)?,
            Self::AboveSurface => p.y >= p.min_surface_level,
            Self::Hole => p.surface_depth <= 0,
            Self::Steep => p.gradient_x <= -4 || p.gradient_z >= 4,
            Self::Noise {
                name,
                min,
                max,
                is_3d,
            } => {
                let n = inputs.noise(name, *is_3d, p)?;
                n >= *min && n <= *max
            }
            Self::Gradient { name, low, high } => {
                let low = low.resolve(p);
                let high = high.resolve(p);
                if p.y <= low {
                    true
                } else if p.y >= high {
                    false
                } else {
                    let t = (f64::from(p.y) - f64::from(low)) / (f64::from(high) - f64::from(low));
                    f64::from(inputs.positional_random(name, p)?) < 1.0 + t * (0.0 - 1.0)
                }
            }
            Self::Y {
                anchor,
                multiplier,
                add_stone,
            } => {
                let y = p.y.wrapping_add(if *add_stone { p.stone_above } else { 0 });
                let limit = anchor
                    .resolve(p)
                    .wrapping_add(p.surface_depth.wrapping_mul(*multiplier));
                y >= limit
            }
            Self::Water {
                offset,
                multiplier,
                add_stone,
            } => {
                p.water_height == i32::MIN
                    || p.y.wrapping_add(if *add_stone { p.stone_above } else { 0 })
                        >= p.water_height
                            .wrapping_add(*offset)
                            .wrapping_add(p.surface_depth.wrapping_mul(*multiplier))
            }
            Self::Stone {
                offset,
                add_surface,
                secondary,
                ceiling,
            } => {
                let depth = if *ceiling {
                    p.stone_below
                } else {
                    p.stone_above
                };
                let add = if *add_surface { p.surface_depth } else { 0 };
                let secondary = if *secondary == 0 {
                    0
                } else {
                    (((p.surface_secondary - (-1.0)) / (1.0 - (-1.0))) * f64::from(*secondary))
                        as i32
                };
                depth
                    <= 1i32
                        .wrapping_add(*offset)
                        .wrapping_add(add)
                        .wrapping_add(secondary)
            }
            Self::Not(condition) => !condition.test(p, inputs)?,
        })
    }
}

enum RuleNode {
    Block(Arc<Value>),
    Sequence(Vec<Arc<RuleNode>>),
    Conditional(Arc<Condition>, Arc<RuleNode>),
    Special(&'static str, Value),
}
impl RuleNode {
    fn apply(
        &self,
        p: &SurfacePosition,
        inputs: &mut impl SurfaceInputs,
    ) -> Result<Option<Arc<Value>>> {
        match self {
            Self::Block(state) => Ok(Some(Arc::clone(state))),
            Self::Sequence(sequence) => {
                for rule in sequence {
                    if let Some(state) = rule.apply(p, inputs)? {
                        return Ok(Some(state));
                    }
                }
                Ok(None)
            }
            Self::Conditional(condition, rule) => {
                if condition.test(p, inputs)? {
                    rule.apply(p, inputs)
                } else {
                    Ok(None)
                }
            }
            Self::Special(kind, definition) => inputs.special_rule(kind, definition, p),
        }
    }
}

pub struct SurfaceRule(Arc<RuleNode>);
impl SurfaceRule {
    pub fn evaluate(
        &self,
        position: &SurfacePosition,
        inputs: &mut impl SurfaceInputs,
    ) -> Result<Option<Arc<Value>>> {
        self.0.apply(position, inputs)
    }
}

/// Resolves active data-pack rule and condition registries, sharing references.
pub struct SurfaceRuleCompiler<'a, P> {
    provider: &'a mut P,
    rules: BTreeMap<ResourceId, Arc<RuleNode>>,
    conditions: BTreeMap<ResourceId, Arc<Condition>>,
    active: BTreeSet<(bool, ResourceId)>,
    nodes: usize,
}
impl<'a, P: ResourceProvider> SurfaceRuleCompiler<'a, P> {
    pub fn new(provider: &'a mut P) -> Self {
        Self {
            provider,
            rules: BTreeMap::new(),
            conditions: BTreeMap::new(),
            active: BTreeSet::new(),
            nodes: 0,
        }
    }
    pub fn compile(&mut self, value: &Value) -> Result<SurfaceRule> {
        Ok(SurfaceRule(self.rule(value, 0)?))
    }
    fn enter(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        ensure!(
            depth <= MAX_DEPTH && self.nodes <= MAX_NODES,
            "material rule exceeds complexity limit"
        );
        Ok(())
    }
    fn reference(&mut self, name: &str, is_rule: bool) -> Result<Value> {
        let id = ResourceId::parse(name)?;
        ensure!(
            self.active.insert((is_rule, id.clone())),
            "cyclic material registry reference: {name}"
        );
        let path = format!(
            "data/{}/worldgen/material_{}/{}.json",
            id.namespace,
            if is_rule { "rule" } else { "condition" },
            id.path
        );
        let result = (|| {
            let bytes = self
                .provider
                .read_data(&path)?
                .with_context(|| format!("missing material definition {name}"))?;
            ensure!(
                bytes.len() <= 16 * 1024 * 1024,
                "material definition too large"
            );
            Ok(serde_json::from_slice(&bytes)?)
        })();
        if result.is_err() {
            self.active.remove(&(is_rule, id));
        }
        result
    }
    fn rule(&mut self, value: &Value, depth: usize) -> Result<Arc<RuleNode>> {
        self.enter(depth)?;
        if let Some(name) = value.as_str() {
            let id = ResourceId::parse(name)?;
            if let Some(rule) = self.rules.get(&id) {
                return Ok(Arc::clone(rule));
            }
            let def = self.reference(name, true)?;
            let result = self.rule(&def, depth + 1);
            self.active.remove(&(true, id.clone()));
            let rule = result.with_context(|| format!("in material rule {name}"))?;
            self.rules.insert(id, Arc::clone(&rule));
            return Ok(rule);
        }
        let obj = value
            .as_object()
            .context("material rule must be an object or identifier")?;
        let kind = kind(value)?;
        let node = match kind.path.as_str() {
            "block" => {
                let state = obj.get("result_state").context("missing result_state")?;
                let name = state
                    .as_str()
                    .or_else(|| state.get("id").and_then(Value::as_str))
                    .context("invalid block state")?;
                ResourceId::parse(name)?;
                RuleNode::Block(Arc::new(state.clone()))
            }
            "sequence" => {
                let seq = obj
                    .get("sequence")
                    .and_then(Value::as_array)
                    .context("invalid material sequence")?;
                RuleNode::Sequence(
                    seq.iter()
                        .map(|v| self.rule(v, depth + 1))
                        .collect::<Result<_>>()?,
                )
            }
            "condition" => RuleNode::Conditional(
                self.condition(required(value, "if_true")?, depth + 1)?,
                self.rule(required(value, "then_run")?, depth + 1)?,
            ),
            "bandlands" => RuleNode::Special("bandlands", value.clone()),
            "ore_vein" => RuleNode::Special("ore_vein", value.clone()),
            _ => bail!(
                "unsupported material rule type {}:{}",
                kind.namespace,
                kind.path
            ),
        };
        Ok(Arc::new(node))
    }
    fn condition(&mut self, value: &Value, depth: usize) -> Result<Arc<Condition>> {
        self.enter(depth)?;
        if let Some(name) = value.as_str() {
            let id = ResourceId::parse(name)?;
            if let Some(condition) = self.conditions.get(&id) {
                return Ok(Arc::clone(condition));
            }
            let def = self.reference(name, false)?;
            let result = self.condition(&def, depth + 1);
            self.active.remove(&(false, id.clone()));
            let condition = result.with_context(|| format!("in material condition {name}"))?;
            self.conditions.insert(id, Arc::clone(&condition));
            return Ok(condition);
        }
        let kind = kind(value)?;
        let condition = match kind.path.as_str() {
            "biome" => Condition::Biome(required(value, "biome_is")?.clone()),
            "temperature" => Condition::Temperature,
            "above_preliminary_surface" => Condition::AboveSurface,
            "hole" => Condition::Hole,
            "steep" => Condition::Steep,
            "not" => Condition::Not(self.condition(required(value, "invert")?, depth + 1)?),
            "noise_threshold" => Condition::Noise {
                name: identifier(value, "noise")?,
                min: number(value, "min_threshold")?,
                max: number(value, "max_threshold")?,
                is_3d: value
                    .get("is_3d")
                    .map(|v| v.as_bool().context("invalid is_3d"))
                    .transpose()?
                    .unwrap_or(false),
            },
            "vertical_gradient" => Condition::Gradient {
                name: identifier(value, "random_name")?,
                low: Anchor::parse(required(value, "true_at_and_below")?)?,
                high: Anchor::parse(required(value, "false_at_and_above")?)?,
            },
            "y_above" => Condition::Y {
                anchor: Anchor::parse(required(value, "anchor")?)?,
                multiplier: integer(value, "surface_depth_multiplier")?,
                add_stone: boolean(value, "add_stone_depth")?,
            },
            "water" => Condition::Water {
                offset: integer(value, "offset")?,
                multiplier: integer(value, "surface_depth_multiplier")?,
                add_stone: boolean(value, "add_stone_depth")?,
            },
            "stone_depth" => Condition::Stone {
                offset: integer(value, "offset")?,
                add_surface: boolean(value, "add_surface_depth")?,
                secondary: integer(value, "secondary_depth_range")?,
                ceiling: match required(value, "surface_type")?.as_str() {
                    Some("floor") => false,
                    Some("ceiling") => true,
                    _ => bail!("invalid surface_type"),
                },
            },
            _ => bail!(
                "unsupported material condition type {}:{}",
                kind.namespace,
                kind.path
            ),
        };
        Ok(Arc::new(condition))
    }
}
fn required<'a>(value: &'a Value, name: &str) -> Result<&'a Value> {
    value.get(name).with_context(|| format!("missing {name}"))
}
fn integer(value: &Value, name: &str) -> Result<i32> {
    i32::try_from(
        required(value, name)?
            .as_i64()
            .with_context(|| format!("invalid integer {name}"))?,
    )
    .with_context(|| format!("integer {name} out of range"))
}
fn boolean(value: &Value, name: &str) -> Result<bool> {
    required(value, name)?
        .as_bool()
        .with_context(|| format!("invalid boolean {name}"))
}
fn number(value: &Value, name: &str) -> Result<f64> {
    required(value, name)?
        .as_f64()
        .with_context(|| format!("invalid number {name}"))
}
fn identifier(value: &Value, name: &str) -> Result<ResourceId> {
    ResourceId::parse(
        required(value, name)?
            .as_str()
            .with_context(|| format!("invalid identifier {name}"))?,
    )
}
fn kind(value: &Value) -> Result<ResourceId> {
    let id = identifier(value, "type")?;
    ensure!(
        id.namespace == "minecraft",
        "unsupported material type namespace {}",
        id.namespace
    );
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Data(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }
    struct Inputs;
    impl SurfaceInputs for Inputs {
        fn biome_matches(&mut self, _: &Value, _: &SurfacePosition) -> Result<bool> {
            Ok(false)
        }
        fn cold_enough_to_snow(&mut self, _: &SurfacePosition) -> Result<bool> {
            Ok(false)
        }
        fn noise(&mut self, _: &ResourceId, _: bool, _: &SurfacePosition) -> Result<f64> {
            Ok(0.0)
        }
        fn positional_random(&mut self, _: &ResourceId, _: &SurfacePosition) -> Result<f32> {
            Ok(0.5)
        }
        fn special_rule(
            &mut self,
            _: &str,
            _: &Value,
            _: &SurfacePosition,
        ) -> Result<Option<Arc<Value>>> {
            bail!("special rules unavailable")
        }
    }
    fn position() -> SurfacePosition {
        SurfacePosition {
            x: 0,
            y: 5,
            z: 0,
            min_y: -64,
            height: 384,
            sea_level: 63,
            stone_above: 1,
            stone_below: 2,
            surface_depth: 3,
            surface_secondary: 0.0,
            water_height: i32::MIN,
            min_surface_level: 0,
            gradient_x: 0,
            gradient_z: 0,
        }
    }
    #[test]
    fn registered_rules_are_ordered_and_cycles_fail() {
        let mut data=Data(BTreeMap::from([
            ("data/minecraft/worldgen/material_rule/first.json".into(),serde_json::to_vec(&json!({"type":"condition","if_true":"minecraft:floor","then_run":{"type":"block","result_state":"minecraft:grass_block"}})).unwrap()),
            ("data/minecraft/worldgen/material_condition/floor.json".into(),serde_json::to_vec(&json!({"type":"stone_depth","offset":0,"add_surface_depth":false,"secondary_depth_range":0,"surface_type":"floor"})).unwrap()),
            ("data/minecraft/worldgen/material_rule/cycle.json".into(),br#""minecraft:cycle""#.to_vec()),
        ]));
        let mut compiler = SurfaceRuleCompiler::new(&mut data);
        let rule=compiler.compile(&json!({"type":"sequence","sequence":["minecraft:first",{"type":"block","result_state":"minecraft:dirt"}]})).unwrap();
        assert_eq!(
            *rule.evaluate(&position(), &mut Inputs).unwrap().unwrap(),
            json!("minecraft:grass_block")
        );
        let mut p = position();
        p.stone_above = 2;
        assert_eq!(
            *rule.evaluate(&p, &mut Inputs).unwrap().unwrap(),
            json!("minecraft:dirt")
        );
        assert!(compiler.compile(&json!("minecraft:cycle")).is_err());
        assert!(compiler.compile(&json!({"type":"other:unknown"})).is_err());
    }
}
