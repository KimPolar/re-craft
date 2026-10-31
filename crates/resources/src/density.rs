//! A subset of 26.3 density functions, optionally bound to a world seed.
//!
//! Unsupported functions are errors rather than substitutes that silently
//! produce a different world.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_core::{
    normal_noise::{
        LegacyNoiseFactory, NoiseSeedFactory, NoiseSeedSource, NormalNoise, XoroshiroNoiseFactory,
    },
    random::{LegacyRandom, XoroshiroRandom},
};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};
mod interval;
use interval::Interval;

const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 100_000;

/// An immutable graph. Referenced registry entries are shared across the graph.
#[derive(Debug)]
pub struct DensityFunction(Node, usize, usize, bool, Interval);

#[derive(Debug)]
enum Node {
    Constant(f32),
    Reference(Arc<DensityFunction>),
    Gradient {
        axis: usize,
        from: i32,
        to: i32,
        value: f32,
        to_value: f32,
        factor: f32,
    },
    Binary {
        op: Binary,
        left: Arc<DensityFunction>,
        right: Arc<DensityFunction>,
    },
    Extremum {
        minimum: bool,
        plan: ExtremumPlan,
        left: Arc<DensityFunction>,
        right: Arc<DensityFunction>,
    },
    Unary {
        op: Unary,
        input: Arc<DensityFunction>,
    },
    Range {
        input: Arc<DensityFunction>,
        min: f32,
        max: f32,
        inside: Arc<DensityFunction>,
        outside: Arc<DensityFunction>,
    },
    Lerp {
        alpha: Arc<DensityFunction>,
        first: Arc<DensityFunction>,
        second: Arc<DensityFunction>,
    },
    Interpolated {
        input: Arc<DensityFunction>,
        cell_xz: i32,
        cell_y: i32,
    },
    Noise {
        noise: Arc<NormalNoise>,
        xz_scale: f64,
        y_scale: f64,
        shifts: [Arc<DensityFunction>; 3],
        shift_mode: u8,
    },
}

#[derive(Clone, Copy, Debug)]
enum Binary {
    Add,
    Sub,
    Mul,
}
#[derive(Clone, Copy, Debug)]
enum ExtremumPlan {
    Left,
    Right,
    LeftConstant,
    RightConstant,
    General(f32),
}
#[derive(Clone, Copy, Debug)]
enum Unary {
    Abs,
    Square,
    Cube,
    HalfNegative,
    QuarterNegative,
    Negate,
    Squeeze,
}

impl DensityFunction {
    /// The interval reported by Java's density function before compilation.
    pub fn range(&self) -> (f32, f32) {
        (self.4.min, self.4.max)
    }
    /// Samples using the single-precision operations used by the 26.3 sampler.
    pub fn sample(&self, x: i32, y: i32, z: i32) -> f32 {
        self.evaluate(x, y, z, false)
    }

    // Java's volume and single-point samplers differ in their multiplication
    // fast path. InterpolatedFunction samples its eight corners in a volume.
    fn evaluate(&self, x: i32, y: i32, z: i32, volume: bool) -> f32 {
        match &self.0 {
            Node::Constant(v) => *v,
            Node::Reference(v) => v.evaluate(x, y, z, volume),
            Node::Gradient {
                axis,
                from,
                to,
                value,
                factor,
                ..
            } => {
                let coordinate = [x, y, z][*axis].clamp((*from).min(*to), (*from).max(*to));
                *value + (coordinate.wrapping_sub(*from) as f32) * *factor
            }
            Node::Binary { op, left, right } => {
                let a = left.evaluate(x, y, z, volume);
                let left_constant = matches!(left.0, Node::Constant(_));
                let right_constant = matches!(right.0, Node::Constant(_));
                if !volume
                    && matches!(op, Binary::Mul)
                    && !left_constant
                    && !right_constant
                    && a == 0.0
                {
                    return 0.0;
                }
                let b = right.evaluate(x, y, z, volume);
                match op {
                    Binary::Add if left_constant => b + a,
                    Binary::Add => a + b,
                    Binary::Sub if !left_constant && right_constant => a + -b,
                    Binary::Sub => a - b,
                    Binary::Mul => a * b,
                }
            }
            Node::Extremum {
                minimum,
                plan,
                left,
                right,
            } => {
                let (a, b) = match plan {
                    ExtremumPlan::Left => return left.evaluate(x, y, z, volume),
                    ExtremumPlan::Right => return right.evaluate(x, y, z, volume),
                    ExtremumPlan::LeftConstant => (
                        right.evaluate(x, y, z, volume),
                        left.evaluate(x, y, z, volume),
                    ),
                    ExtremumPlan::RightConstant => (
                        left.evaluate(x, y, z, volume),
                        right.evaluate(x, y, z, volume),
                    ),
                    ExtremumPlan::General(bound) => {
                        let a = left.evaluate(x, y, z, volume);
                        if !volume && if *minimum { a <= *bound } else { a >= *bound } {
                            return a;
                        }
                        (a, right.evaluate(x, y, z, volume))
                    }
                };
                if *minimum {
                    interval::min(a, b)
                } else {
                    interval::max(a, b)
                }
            }
            Node::Unary { op, input } => {
                let a = input.evaluate(x, y, z, volume);
                match op {
                    Unary::Abs => a.abs(),
                    Unary::Square => a * a,
                    Unary::Cube => a * a * a,
                    Unary::HalfNegative => {
                        if a > 0.0 {
                            a
                        } else {
                            a * 0.5
                        }
                    }
                    Unary::QuarterNegative => {
                        if a > 0.0 {
                            a
                        } else {
                            a * 0.25
                        }
                    }
                    Unary::Negate => -a,
                    Unary::Squeeze => squeeze(a),
                }
            }
            Node::Range {
                input,
                min,
                max,
                inside,
                outside,
            } => {
                let v = input.evaluate(x, y, z, volume);
                if v >= *min && v < *max {
                    inside.evaluate(x, y, z, volume)
                } else {
                    outside.evaluate(x, y, z, volume)
                }
            }
            Node::Lerp {
                alpha,
                first,
                second,
            } => {
                let alpha = alpha.evaluate(x, y, z, volume);
                if alpha == 0.0 {
                    return first.evaluate(x, y, z, volume);
                }
                if alpha == 1.0 {
                    return second.evaluate(x, y, z, volume);
                }
                lerp(
                    alpha,
                    first.evaluate(x, y, z, volume),
                    second.evaluate(x, y, z, volume),
                )
            }
            Node::Interpolated {
                input,
                cell_xz,
                cell_y,
            } => {
                debug_assert!(!volume, "nested interpolation is not supported");
                let rx = x.rem_euclid(*cell_xz);
                let ry = y.rem_euclid(*cell_y);
                let rz = z.rem_euclid(*cell_xz);
                if rx == 0 && ry == 0 && rz == 0 {
                    return input.evaluate(x, y, z, false);
                }
                let (x0, y0, z0) = (x.wrapping_sub(rx), y.wrapping_sub(ry), z.wrapping_sub(rz));
                let (x1, y1, z1) = (
                    x0.wrapping_add(*cell_xz),
                    y0.wrapping_add(*cell_y),
                    z0.wrapping_add(*cell_xz),
                );
                let (tx, ty, tz) = (
                    rx as f32 / *cell_xz as f32,
                    ry as f32 / *cell_y as f32,
                    rz as f32 / *cell_xz as f32,
                );
                let corner = |x, y, z| input.evaluate(x, y, z, true);
                let z0_plane = lerp(
                    ty,
                    lerp(tx, corner(x0, y0, z0), corner(x1, y0, z0)),
                    lerp(tx, corner(x0, y1, z0), corner(x1, y1, z0)),
                );
                let z1_plane = lerp(
                    ty,
                    lerp(tx, corner(x0, y0, z1), corner(x1, y0, z1)),
                    lerp(tx, corner(x0, y1, z1), corner(x1, y1, z1)),
                );
                lerp(tz, z0_plane, z1_plane)
            }
            Node::Noise {
                noise,
                xz_scale,
                y_scale,
                shifts,
                shift_mode,
            } => {
                if *shift_mode == 0 && volume {
                    return noise.sample_scaled_volume(x, y, z, *xz_scale, *y_scale);
                }
                let mut px = f64::from(x) * xz_scale;
                let mut py = f64::from(y) * y_scale;
                let mut pz = f64::from(z) * xz_scale;
                if *shift_mode != 0 {
                    px += f64::from(shifts[0].evaluate(x, y, z, volume));
                    pz += f64::from(shifts[2].evaluate(x, y, z, volume));
                    if *shift_mode == 2 {
                        py += f64::from(shifts[1].evaluate(x, y, z, volume));
                    }
                }
                noise.sample(px, py, pz)
            }
        }
    }
}

fn lerp(alpha: f32, first: f32, second: f32) -> f32 {
    first + alpha * (second - first)
}
fn squeeze(a: f32) -> f32 {
    let v = if a < -1.0 {
        -1.0
    } else if a > 1.0 {
        1.0
    } else {
        a
    };
    v / 2.0 - v * v * v / 24.0
}

fn make(node: Node) -> Result<Arc<DensityFunction>> {
    let children: Vec<&Arc<DensityFunction>> = match &node {
        Node::Constant(_) | Node::Gradient { .. } => vec![],
        Node::Reference(value)
        | Node::Unary { input: value, .. }
        | Node::Interpolated { input: value, .. } => vec![value],
        Node::Binary { left, right, .. } | Node::Extremum { left, right, .. } => vec![left, right],
        Node::Range {
            input,
            inside,
            outside,
            ..
        } => vec![input, inside, outside],
        Node::Lerp {
            alpha,
            first,
            second,
        } => vec![alpha, first, second],
        Node::Noise { shifts, .. } => shifts.iter().collect(),
    };
    let depth = 1 + children.iter().map(|child| child.1).max().unwrap_or(0);
    let mut work = children
        .iter()
        .fold(1usize, |sum, child| sum.saturating_add(child.2));
    if let Node::Interpolated { input, .. } = &node {
        ensure!(
            !input.3,
            "nested interpolated density functions are not supported"
        );
        work = 1usize.saturating_add(input.2.saturating_mul(8));
    }
    let has_interpolation =
        matches!(node, Node::Interpolated { .. }) || children.iter().any(|child| child.3);
    ensure!(depth <= MAX_DEPTH, "density function exceeds maximum depth");
    ensure!(
        work <= MAX_NODES,
        "density function exceeds maximum sampling work"
    );
    let range = match &node {
        Node::Constant(value) => Interval::exact(*value),
        Node::Reference(input) | Node::Interpolated { input, .. } => input.4,
        Node::Gradient {
            value, to_value, ..
        } => Interval::enclosing_values(*value, *to_value),
        Node::Binary { op, left, right } => match op {
            Binary::Add => left.4.add(right.4),
            Binary::Sub => left.4.sub(right.4),
            Binary::Mul => left.4.mul(right.4),
        },
        Node::Extremum {
            minimum,
            left,
            right,
            ..
        } => {
            if *minimum {
                left.4.minimum(right.4)
            } else {
                left.4.maximum(right.4)
            }
        }
        Node::Unary { op, input } => match op {
            Unary::Abs => input.4.abs(),
            Unary::Square => input.4.square(),
            Unary::Cube => input.4.monotonic(|v| v * v * v),
            Unary::HalfNegative => input.4.monotonic(|v| if v > 0.0 { v } else { v * 0.5 }),
            Unary::QuarterNegative => input.4.monotonic(|v| if v > 0.0 { v } else { v * 0.25 }),
            Unary::Negate => Interval::exact(0.0).sub(input.4),
            Unary::Squeeze => input.4.monotonic(squeeze),
        },
        Node::Range {
            inside, outside, ..
        } => Interval::enclosing(inside.4, outside.4),
        Node::Lerp {
            alpha,
            first,
            second,
        } => alpha.4.lerp(first.4, second.4),
        Node::Noise { noise, .. } => {
            let (min, max) = noise.range();
            Interval::bounds(min, max)
        }
    };
    Ok(Arc::new(DensityFunction(
        node,
        depth,
        work,
        has_interpolation,
        range,
    )))
}

/// Resolves functions from the active data-pack stack, including its overrides.
pub struct DensityCompiler<'a, P: ResourceProvider> {
    resources: &'a mut P,
    cache: BTreeMap<ResourceId, Arc<DensityFunction>>,
    active: BTreeSet<ResourceId>,
    nodes: usize,
    noise_factory: Option<WorldNoiseFactory>,
    world_seed: Option<i64>,
    noises: BTreeMap<ResourceId, Arc<NormalNoise>>,
}

enum WorldNoiseFactory {
    Legacy(LegacyNoiseFactory),
    Xoroshiro(XoroshiroNoiseFactory),
}

/// Random source selected by the world's 26.3 generator settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldgenRandom {
    Legacy,
    Xoroshiro,
}

impl<'a, P: ResourceProvider> DensityCompiler<'a, P> {
    pub fn new(resources: &'a mut P) -> Self {
        Self {
            resources,
            cache: BTreeMap::new(),
            active: BTreeSet::new(),
            nodes: 0,
            noise_factory: None,
            world_seed: None,
            noises: BTreeMap::new(),
        }
    }

    pub fn with_world_seed(resources: &'a mut P, seed: i64, algorithm: WorldgenRandom) -> Self {
        let noise_factory = match algorithm {
            WorldgenRandom::Legacy => {
                WorldNoiseFactory::Legacy(LegacyRandom::new(seed).fork_noise_factory())
            }
            WorldgenRandom::Xoroshiro => {
                WorldNoiseFactory::Xoroshiro(XoroshiroRandom::new(seed).fork_noise_factory())
            }
        };
        let mut compiler = Self::new(resources);
        compiler.noise_factory = Some(noise_factory);
        compiler.world_seed = Some(seed);
        compiler
    }

    fn noise(&mut self, name: &str) -> Result<Arc<NormalNoise>> {
        let id = ResourceId::parse(name)?;
        if let Some(noise) = self.noises.get(&id) {
            return Ok(Arc::clone(noise));
        }
        ensure!(
            self.noise_factory.is_some(),
            "a world seed is required for density noise"
        );
        let canonical = format!("{}:{}", id.namespace, id.path);
        let parameters = crate::noise::load_normal_noise(self.resources, &canonical)?;
        let nether_offset = if id.namespace == "minecraft" {
            match id.path.as_str() {
                "nether/temperature" => Some(0),
                "nether/vegetation" => Some(1),
                _ => None,
            }
        } else {
            None
        };
        let noise = if let Some(offset) = nether_offset {
            let seed = self.world_seed.unwrap().wrapping_add(offset);
            parameters.create_legacy_nether(&mut LegacyRandom::new(seed))
        } else {
            match self.noise_factory.as_ref().unwrap() {
                WorldNoiseFactory::Legacy(factory) => {
                    parameters.create(&mut factory.random_for(&canonical))
                }
                WorldNoiseFactory::Xoroshiro(factory) => {
                    parameters.create(&mut factory.random_for(&canonical))
                }
            }
        }
        .map_err(anyhow::Error::msg)?;
        let noise = Arc::new(noise);
        self.noises.insert(id, Arc::clone(&noise));
        Ok(noise)
    }

    pub fn compile_id(&mut self, id: &str) -> Result<Arc<DensityFunction>> {
        self.reference(id, 0)
    }

    pub fn compile(&mut self, value: &Value) -> Result<Arc<DensityFunction>> {
        self.expression(value, 0)
    }

    fn reference(&mut self, name: &str, depth: usize) -> Result<Arc<DensityFunction>> {
        ensure!(depth <= MAX_DEPTH, "density function exceeds maximum depth");
        let id = ResourceId::parse(name)?;
        if let Some(function) = self.cache.get(&id) {
            return Ok(Arc::clone(function));
        }
        ensure!(
            self.active.insert(id.clone()),
            "cyclic density function reference: {name}"
        );
        let result = (|| {
            let path = format!(
                "data/{}/worldgen/density_function/{}.json",
                id.namespace, id.path
            );
            let bytes = self
                .resources
                .read_data(&path)?
                .with_context(|| format!("missing density function {name}"))?;
            ensure!(
                bytes.len() <= 16 * 1024 * 1024,
                "density function is too large: {name}"
            );
            let value = serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid density function JSON: {name}"))?;
            self.expression(&value, depth + 1)
                .with_context(|| format!("in density function {name}"))
        })();
        self.active.remove(&id);
        // The Java registry keeps a holder around references. Binary samplers
        // only use their constant fast path for literal ConstantFunction nodes.
        let function = make(Node::Reference(result?))?;
        self.cache.insert(id, Arc::clone(&function));
        Ok(function)
    }

    fn expression(&mut self, value: &Value, depth: usize) -> Result<Arc<DensityFunction>> {
        ensure!(depth <= MAX_DEPTH, "density function exceeds maximum depth");
        if let Some(name) = value.as_str() {
            return self.reference(name, depth + 1);
        }
        self.nodes += 1;
        ensure!(
            self.nodes <= MAX_NODES,
            "density function exceeds maximum node count"
        );
        if value.is_number() {
            return make(Node::Constant(number(value)?));
        }
        let object = value
            .as_object()
            .context("density function must be a number, reference, or object")?;
        let field = |name: &str| {
            object
                .get(name)
                .with_context(|| format!("missing density field {name}"))
        };
        let kind = field("type")?
            .as_str()
            .context("density type must be a string")?;
        let id = ResourceId::parse(kind)?;
        ensure!(
            id.namespace == "minecraft",
            "unsupported density function type {kind}"
        );
        let node = match id.path.as_str() {
            "constant" => Node::Constant(number(field("value")?)?),
            "gradient" => {
                let axis = match field("axis")?.as_str() {
                    Some("x") => 0,
                    Some("y") => 1,
                    Some("z") => 2,
                    _ => bail!("invalid gradient axis"),
                };
                if let Some(tiling) = object.get("tiling") {
                    ensure!(
                        tiling.as_str() == Some("clamp_to_edge"),
                        "unsupported gradient tiling: {tiling}"
                    );
                }
                let from = integer(field("from_coordinate")?)?;
                let to = integer(field("to_coordinate")?)?;
                ensure!(from != to, "gradient coordinates must differ");
                let from_value = number(field("from_value")?)?;
                let to_value = number(field("to_value")?)?;
                Node::Gradient {
                    axis,
                    from,
                    to,
                    value: from_value,
                    to_value,
                    factor: (to_value - from_value) / (to.wrapping_sub(from) as f32),
                }
            }
            "add" | "sub" | "mul" => {
                let op = match id.path.as_str() {
                    "add" => Binary::Add,
                    "sub" => Binary::Sub,
                    "mul" => Binary::Mul,
                    _ => Binary::Mul,
                };
                Node::Binary {
                    op,
                    left: self.expression(field("left")?, depth + 1)?,
                    right: self.expression(field("right")?, depth + 1)?,
                }
            }
            "min" | "max" => {
                let left = self.expression(field("left")?, depth + 1)?;
                let right = self.expression(field("right")?, depth + 1)?;
                let minimum = id.path == "min";
                let plan = if if minimum {
                    left.4.max < right.4.min
                } else {
                    left.4.min > right.4.max
                } {
                    ExtremumPlan::Left
                } else if if minimum {
                    right.4.max < left.4.min
                } else {
                    right.4.min > left.4.max
                } {
                    ExtremumPlan::Right
                } else if matches!(left.0, Node::Constant(_)) {
                    ExtremumPlan::LeftConstant
                } else if matches!(right.0, Node::Constant(_)) {
                    ExtremumPlan::RightConstant
                } else {
                    ExtremumPlan::General(if minimum { right.4.min } else { right.4.max })
                };
                Node::Extremum {
                    minimum,
                    plan,
                    left,
                    right,
                }
            }
            "abs" | "square" | "cube" | "half_negative" | "quarter_negative" | "negate"
            | "squeeze" => {
                let op = match id.path.as_str() {
                    "abs" => Unary::Abs,
                    "square" => Unary::Square,
                    "cube" => Unary::Cube,
                    "half_negative" => Unary::HalfNegative,
                    "quarter_negative" => Unary::QuarterNegative,
                    "negate" => Unary::Negate,
                    _ => Unary::Squeeze,
                };
                Node::Unary {
                    op,
                    input: self.expression(field("input")?, depth + 1)?,
                }
            }
            "range_choice" => Node::Range {
                input: self.expression(field("input")?, depth + 1)?,
                min: number(field("min_inclusive")?)?,
                max: number(field("max_exclusive")?)?,
                inside: self.expression(field("when_in_range")?, depth + 1)?,
                outside: self.expression(field("when_out_of_range")?, depth + 1)?,
            },
            "lerp" => Node::Lerp {
                alpha: self.expression(field("alpha")?, depth + 1)?,
                first: self.expression(field("first")?, depth + 1)?,
                second: self.expression(field("second")?, depth + 1)?,
            },
            "interpolated" => {
                let cell_xz = integer(field("cell_size_xz")?)?;
                let cell_y = integer(field("cell_size_y")?)?;
                ensure!(
                    cell_xz > 0 && cell_y > 0,
                    "interpolation cell sizes must be positive"
                );
                Node::Interpolated {
                    input: self.expression(field("input")?, depth + 1)?,
                    cell_xz,
                    cell_y,
                }
            }
            "noise" => {
                let name = field("noise")?
                    .as_str()
                    .context("noise must reference a registered noise definition")?;
                let noise = self.noise(name)?;
                let xz_scale = field("xz_scale")?
                    .as_f64()
                    .context("xz_scale must be a number")?;
                let y_scale = field("y_scale")?
                    .as_f64()
                    .context("y_scale must be a number")?;
                ensure!(
                    xz_scale.is_finite() && y_scale.is_finite(),
                    "noise scale must be finite"
                );
                let mut shifts = Vec::with_capacity(3);
                for key in ["shift_x", "shift_y", "shift_z"] {
                    shifts.push(match object.get(key) {
                        Some(value) => self.expression(value, depth + 1)?,
                        None => make(Node::Constant(0.0))?,
                    });
                }
                let zero = |function: &Arc<DensityFunction>| matches!(&function.0, Node::Constant(value) if value.to_bits() == 0);
                let shift_mode = if shifts.iter().all(zero) {
                    0
                } else if zero(&shifts[1]) {
                    1
                } else {
                    2
                };
                Node::Noise {
                    noise,
                    xz_scale,
                    y_scale,
                    shifts: shifts.try_into().unwrap(),
                    shift_mode,
                }
            }
            _ => bail!("unsupported density function type {kind}"),
        };
        make(node)
    }
}

fn number(value: &Value) -> Result<f32> {
    let v = value.as_f64().context("density value must be a number")? as f32;
    ensure!(
        v.is_finite() && (-1_000_000.0..=1_000_000.0).contains(&v),
        "density value is outside the 26.3 range"
    );
    Ok(v)
}
fn integer(value: &Value) -> Result<i32> {
    i32::try_from(
        value
            .as_i64()
            .context("gradient coordinate must be an integer")?,
    )
    .context("gradient coordinate outside i32 range")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    struct Data(BTreeMap<String, Vec<u8>>);
    impl Data {
        fn insert(&mut self, id: &str, value: Value) {
            self.0.insert(
                format!("data/test/worldgen/density_function/{id}.json"),
                serde_json::to_vec(&value).unwrap(),
            );
        }
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }

    #[test]
    fn resolves_and_shares_data_pack_references() {
        let mut data = Data::default();
        data.insert("value", json!(0.25));
        data.insert(
            "sum",
            json!({"type":"minecraft:add", "left":"test:value", "right":"test:value"}),
        );
        let mut compiler = DensityCompiler::new(&mut data);
        let sum = compiler.compile_id("test:sum").unwrap();
        assert_eq!(sum.sample(7, 9, -5), 0.5);
        let Node::Reference(sum) = &sum.0 else {
            panic!("expected registry reference");
        };
        if let Node::Binary { left, right, .. } = &sum.0 {
            assert!(Arc::ptr_eq(left, right));
        } else {
            panic!("expected binary function");
        }
    }

    #[test]
    fn rejects_cycles_missing_and_unsupported_functions() {
        let mut data = Data::default();
        data.insert("a", json!("test:b"));
        data.insert("b", json!("test:a"));
        data.insert("noise", json!({"type":"minecraft:old_blended_noise"}));
        let mut compiler = DensityCompiler::new(&mut data);
        assert!(format!("{:#}", compiler.compile_id("test:a").unwrap_err()).contains("cyclic"));
        assert!(
            format!("{:#}", compiler.compile_id("test:missing").unwrap_err()).contains("missing")
        );
        assert!(
            format!("{:#}", compiler.compile_id("test:noise").unwrap_err()).contains("unsupported")
        );
        assert!(compiler.compile(&json!({"type":"gradient","axis":"y","from_coordinate":0,"to_coordinate":0,"from_value":0,"to_value":1})).is_err());
        assert!(compiler.compile(&json!(1000001)).is_err());
        assert!(compiler.compile_id("test:../bad").is_err());
    }

    #[test]
    fn gradient_clamps_and_range_is_half_open() {
        let mut data = Data::default();
        let mut compiler = DensityCompiler::new(&mut data);
        let function = compiler.compile(&json!({"type":"range_choice", "input":{"type":"gradient", "axis":"y", "from_coordinate":0, "to_coordinate":10, "from_value":0, "to_value":1}, "min_inclusive":0.5, "max_exclusive":1, "when_in_range":2, "when_out_of_range":3})).unwrap();
        assert_eq!(function.sample(0, -1, 0), 3.0);
        assert_eq!(function.sample(0, 5, 0), 2.0);
        assert_eq!(function.sample(0, 10, 0), 3.0);
        assert_eq!(function.sample(0, 100, 0), 3.0);
    }

    #[test]
    fn bounds_shared_graph_depth_and_sampling_work() {
        let mut data = Data::default();
        data.insert("node0", json!(1));
        for index in 1..20 {
            let previous = format!("test:node{}", index - 1);
            data.insert(
                &format!("node{index}"),
                json!({"type":"add", "left":previous, "right":previous}),
            );
        }
        let mut compiler = DensityCompiler::new(&mut data);
        assert!(
            format!("{:#}", compiler.compile_id("test:node19").unwrap_err())
                .contains("sampling work")
        );
        assert_eq!(
            compiler
                .compile(&json!({"type":"min", "left":0, "right":1}))
                .unwrap()
                .sample(0, 0, 0),
            0.0
        );
    }

    #[test]
    fn interpolation_validates_cells_and_rejects_nested_volume_sampling() {
        let mut data = Data::default();
        let mut compiler = DensityCompiler::new(&mut data);
        for (xz, y) in [(0, 1), (1, -1)] {
            assert!(compiler.compile(&json!({"type":"interpolated", "input":1, "cell_size_xz":xz, "cell_size_y":y})).is_err());
        }
        let inner = json!({"type":"interpolated", "input":1, "cell_size_xz":4, "cell_size_y":8});
        assert!(compiler.compile(&json!({"type":"interpolated", "input":{"type":"add", "left":inner, "right":1}, "cell_size_xz":4, "cell_size_y":8})).is_err());
    }

    #[test]
    fn extremum_preserves_java_single_and_volume_zero_shortcuts() {
        let mut data = Data::default();
        let mut compiler = DensityCompiler::new(&mut data);
        let positive = json!({"type":"gradient", "axis":"y", "from_coordinate":0, "to_coordinate":1, "from_value":0.0, "to_value":0.0});
        let negative = json!({"type":"negate", "input":0.0});
        let min = compiler
            .compile(&json!({"type":"min", "left":positive, "right":negative}))
            .unwrap();
        assert_eq!(min.sample(0, 0, 0).to_bits(), 0.0f32.to_bits());
        assert_eq!(min.evaluate(0, 0, 0, true).to_bits(), (-0.0f32).to_bits());
        let max = compiler
            .compile(&json!({"type":"max", "left":negative, "right":positive}))
            .unwrap();
        assert_eq!(max.sample(0, 0, 0).to_bits(), (-0.0f32).to_bits());
        assert_eq!(max.evaluate(0, 0, 0, true).to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn seeded_noise_reuses_registered_sampler_and_requires_seed() {
        let mut data = Data::default();
        data.0.insert(
            "data/test/worldgen/noise/sample.json".to_owned(),
            br#"{"base_octave":-3}"#.to_vec(),
        );
        data.0.insert(
            "data/minecraft/worldgen/noise/nether/temperature.json".to_owned(),
            br#"{"base_octave":-7,"octave_count":2}"#.to_vec(),
        );
        let value = json!({"type":"noise","noise":"test:sample","xz_scale":0.013,"y_scale":0.021});
        data.insert("from_pack", value.clone());
        assert!(DensityCompiler::new(&mut data).compile(&value).is_err());
        let mut compiler =
            DensityCompiler::with_world_seed(&mut data, 123, WorldgenRandom::Xoroshiro);
        let first = compiler.compile(&value).unwrap();
        let second = compiler.compile(&value).unwrap();
        if let (Node::Noise { noise: a, .. }, Node::Noise { noise: b, .. }) = (&first.0, &second.0)
        {
            assert!(Arc::ptr_eq(a, b));
        } else {
            panic!("expected noise functions");
        }
        assert_eq!(
            first.sample(10, 20, 30).to_bits(),
            second.sample(10, 20, 30).to_bits()
        );
        let referenced = compiler.compile_id("test:from_pack").unwrap();
        assert_eq!(
            first.sample(10, 20, 30).to_bits(),
            referenced.sample(10, 20, 30).to_bits()
        );
        let nether =
            json!({"type":"noise","noise":"minecraft:nether/temperature","xz_scale":1,"y_scale":1});
        let nether_xor = compiler.compile(&nether).unwrap();
        let mut legacy = DensityCompiler::with_world_seed(&mut data, 123, WorldgenRandom::Legacy);
        let nether_legacy = legacy.compile(&nether).unwrap();
        assert_eq!(
            nether_xor.sample(7, 8, 9).to_bits(),
            nether_legacy.sample(7, 8, 9).to_bits()
        );
    }
}
