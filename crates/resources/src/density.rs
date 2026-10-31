//! The context-independent subset of 26.3 density functions.
//!
//! Unsupported functions are errors: treating noise as a constant would
//! silently produce a different world.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};

const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 100_000;

/// An immutable graph. Referenced registry entries are shared across the graph.
#[derive(Debug)]
pub struct DensityFunction(Node, usize, usize, bool);

#[derive(Debug)]
enum Node {
    Constant(f32),
    Reference(Arc<DensityFunction>),
    Gradient {
        axis: usize,
        from: i32,
        to: i32,
        value: f32,
        factor: f32,
    },
    Binary {
        op: Binary,
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
}

#[derive(Clone, Copy, Debug)]
enum Binary {
    Add,
    Sub,
    Mul,
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
                    Unary::Squeeze => {
                        let v = if a < -1.0 {
                            -1.0
                        } else if a > 1.0 {
                            1.0
                        } else {
                            a
                        };
                        v / 2.0 - v * v * v / 24.0
                    }
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
        }
    }
}

fn lerp(alpha: f32, first: f32, second: f32) -> f32 {
    first + alpha * (second - first)
}

fn make(node: Node) -> Result<Arc<DensityFunction>> {
    let children: Vec<&Arc<DensityFunction>> = match &node {
        Node::Constant(_) | Node::Gradient { .. } => vec![],
        Node::Reference(value)
        | Node::Unary { input: value, .. }
        | Node::Interpolated { input: value, .. } => vec![value],
        Node::Binary { left, right, .. } => vec![left, right],
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
    Ok(Arc::new(DensityFunction(
        node,
        depth,
        work,
        has_interpolation,
    )))
}

/// Resolves functions from the active data-pack stack, including its overrides.
pub struct DensityCompiler<'a, P: ResourceProvider> {
    resources: &'a mut P,
    cache: BTreeMap<ResourceId, Arc<DensityFunction>>,
    active: BTreeSet<ResourceId>,
    nodes: usize,
}

impl<'a, P: ResourceProvider> DensityCompiler<'a, P> {
    pub fn new(resources: &'a mut P) -> Self {
        Self {
            resources,
            cache: BTreeMap::new(),
            active: BTreeSet::new(),
            nodes: 0,
        }
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
        data.insert("noise", json!({"type":"minecraft:noise"}));
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
        assert!(
            compiler
                .compile(&json!({"type":"min", "left":0, "right":1}))
                .is_err()
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
}
