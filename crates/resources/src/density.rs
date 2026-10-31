//! A subset of 26.3 density functions, optionally bound to a world seed.
//!
//! Unsupported functions are errors rather than substitutes that silently
//! produce a different world.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::{Deref, DerefMut},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_core::{
    blended_noise::BlendedNoise,
    normal_noise::{
        LegacyNoiseFactory, NoiseSeedFactory, NoiseSeedSource, NormalNoise, XoroshiroNoiseFactory,
    },
    random::{LegacyRandom, XoroshiroRandom},
    simplex::Simplex2D,
};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};
mod interval;
mod spline;
use interval::Interval;
use spline::Spline;

const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 100_000;
const MAX_SURFACE_STEPS: f64 = 1_000_000.0;

/// An immutable graph. Referenced registry entries are shared across the graph.
#[derive(Debug)]
pub struct DensityFunction(Node, usize, usize, bool, Interval, u8, bool);

#[derive(Clone, Copy)]
struct BlockVolume {
    min: [i32; 3],
    max: [i32; 3],
}

#[derive(Default)]
struct SampleMemo {
    values: HashMap<(usize, i32, i32, i32, bool), f32>,
    volume: Option<BlockVolume>,
}
impl Deref for SampleMemo {
    type Target = HashMap<(usize, i32, i32, i32, bool), f32>;
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
impl DerefMut for SampleMemo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values
    }
}

fn memo_key(
    function: &DensityFunction,
    x: i32,
    y: i32,
    z: i32,
    volume: bool,
) -> (usize, i32, i32, i32, bool) {
    (
        function as *const DensityFunction as usize,
        if function.5 & 1 == 0 { 0 } else { x },
        if function.5 & 2 == 0 { 0 } else { y },
        if function.5 & 4 == 0 { 0 } else { z },
        volume,
    )
}

/// Reuse cache storage across samples; each sample starts with an empty cache.
#[derive(Default)]
pub struct DensityEvaluator {
    memo: SampleMemo,
}

impl DensityEvaluator {
    pub fn sample(&mut self, function: &DensityFunction, x: i32, y: i32, z: i32) -> f32 {
        self.memo.clear();
        self.memo.volume = None;
        function.evaluate_cached(x, y, z, false, &mut self.memo)
    }

    /// Sample related functions at one position, reusing their shared caches.
    pub fn sample_array<const N: usize>(
        &mut self,
        functions: &[Arc<DensityFunction>; N],
        x: i32,
        y: i32,
        z: i32,
    ) -> [f32; N] {
        self.memo.clear();
        self.memo.volume = None;
        std::array::from_fn(|index| {
            functions[index].evaluate_cached(x, y, z, false, &mut self.memo)
        })
    }

    /// Evaluate a unit-step block volume in Java's Z, X, Y buffer order.
    /// Slices enclosing interpolated functions are not yet supported in bulk.
    pub fn sample_block_volume(
        &mut self,
        function: &DensityFunction,
        min: [i32; 3],
        size: [u32; 3],
    ) -> Result<Vec<f32>> {
        ensure!(
            function.6,
            "bulk sampling of a slice enclosing interpolation is not supported"
        );
        ensure!(
            size.iter().all(|n| *n > 0),
            "volume dimensions must be positive"
        );
        let count = size
            .iter()
            .try_fold(1usize, |total, n| total.checked_mul(*n as usize))
            .context("volume size overflow")?;
        ensure!(count <= 1_048_576, "volume exceeds maximum sample count");
        let mut max = [0; 3];
        for axis in 0..3 {
            max[axis] = i64::from(min[axis])
                .checked_add(i64::from(size[axis]) - 1)
                .and_then(|v| i32::try_from(v).ok())
                .context("volume coordinates overflow")?;
        }
        self.memo.volume = Some(BlockVolume { min, max });
        let mut output = Vec::with_capacity(count);
        for z in min[2]..=max[2] {
            for x in min[0]..=max[0] {
                self.memo.clear();
                for y in min[1]..=max[1] {
                    output.push(function.evaluate_cached(x, y, z, true, &mut self.memo));
                }
            }
        }
        self.memo.volume = None;
        Ok(output)
    }
}

#[derive(Debug)]
enum Node {
    Constant(f32),
    Reference(Arc<DensityFunction>),
    Cache(Arc<DensityFunction>),
    Clamp {
        input: Arc<DensityFunction>,
        min: f32,
        max: f32,
    },
    Gradient {
        axis: usize,
        from: i32,
        to: i32,
        value: f32,
        to_value: f32,
        factor: f32,
    },
    DistanceToPoint {
        point: [i32; 3],
        metric: DistanceMetric,
    },
    Slice {
        axis: usize,
        coordinate: i32,
        input: Arc<DensityFunction>,
    },
    FindTopSurface {
        density: Arc<DensityFunction>,
        upper: Arc<DensityFunction>,
        lower: i32,
        cell_height: i32,
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
    IntervalSelect {
        input: Arc<DensityFunction>,
        thresholds: Vec<f32>,
        functions: Vec<Arc<DensityFunction>>,
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
    Shift {
        noise: Arc<NormalNoise>,
        kind: ShiftKind,
    },
    EndOuterIslands(Arc<Simplex2D>),
    OldBlendedNoise(Arc<BlendedNoise>),
    Spline(Spline),
    UnblendedAlpha,
    UnblendedOffset,
    UnblendedDensity(Arc<DensityFunction>),
    NoStructures,
}

#[derive(Clone, Copy, Debug)]
enum Binary {
    Add,
    Sub,
    Mul,
    Div,
}
#[derive(Clone, Copy, Debug)]
enum ShiftKind {
    A,
    B,
    ThreeDimensional,
}
#[derive(Clone, Copy, Debug)]
enum DistanceMetric {
    Euclidean,
    EuclideanSquared,
    Manhattan,
    Chebyshev,
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
        let mut memo = SampleMemo::default();
        self.evaluate_cached(x, y, z, volume, &mut memo)
    }

    fn evaluate_cached(&self, x: i32, y: i32, z: i32, volume: bool, memo: &mut SampleMemo) -> f32 {
        match &self.0 {
            Node::Constant(v) => *v,
            Node::Reference(v) => v.evaluate_cached(x, y, z, volume, memo),
            Node::Cache(input) => {
                let key = memo_key(self, x, y, z, volume);
                if let Some(value) = memo.get(&key) {
                    return *value;
                }
                let value = input.evaluate_cached(x, y, z, volume, memo);
                memo.insert(key, value);
                value
            }
            Node::Clamp { input, min, max } => {
                let value = input.evaluate_cached(x, y, z, volume, memo);
                if value < *min {
                    *min
                } else {
                    interval::min(value, *max)
                }
            }
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
            Node::DistanceToPoint { point, metric } => {
                let dx = point[0].wrapping_sub(x) as f32;
                let dy = point[1].wrapping_sub(y) as f32;
                let dz = point[2].wrapping_sub(z) as f32;
                match metric {
                    DistanceMetric::Euclidean => (dx * dx + dy * dy + dz * dz).sqrt(),
                    DistanceMetric::EuclideanSquared => dx * dx + dy * dy + dz * dz,
                    DistanceMetric::Manhattan => dx.abs() + dy.abs() + dz.abs(),
                    DistanceMetric::Chebyshev => dx.abs().max(dy.abs()).max(dz.abs()),
                }
            }
            Node::Slice {
                axis,
                coordinate,
                input,
            } => {
                let mut position = [x, y, z];
                position[*axis] = *coordinate;
                input.evaluate_cached(position[0], position[1], position[2], volume, memo)
            }
            Node::FindTopSurface {
                density,
                upper,
                lower,
                cell_height,
            } => {
                let x = if self.5 & 1 == 0 { 0 } else { x };
                let z = if self.5 & 4 == 0 { 0 } else { z };
                let upper = upper.evaluate_cached(x, 0, z, volume, memo);
                let start =
                    ((upper / *cell_height as f32).floor() as i32).wrapping_mul(*cell_height);
                if start <= *lower {
                    return *lower as f32;
                }
                let mut y = start;
                while y >= *lower {
                    if density.evaluate_cached(x, y, z, false, memo) > 0.0 {
                        return y as f32;
                    }
                    y = y.wrapping_sub(*cell_height);
                }
                *lower as f32
            }
            Node::Binary { op, left, right } => {
                let a = left.evaluate_cached(x, y, z, volume, memo);
                let left_constant = matches!(left.0, Node::Constant(_));
                let right_constant = matches!(right.0, Node::Constant(_));
                if !volume
                    && matches!(op, Binary::Mul | Binary::Div)
                    && !left_constant
                    && !right_constant
                    && a == 0.0
                {
                    return 0.0;
                }
                let b = right.evaluate_cached(x, y, z, volume, memo);
                match op {
                    Binary::Add if left_constant => b + a,
                    Binary::Add => a + b,
                    Binary::Sub if !left_constant && right_constant => a + -b,
                    Binary::Sub => a - b,
                    Binary::Mul => a * b,
                    Binary::Div if !left_constant && right_constant => a * (1.0 / b),
                    Binary::Div => a / b,
                }
            }
            Node::Extremum {
                minimum,
                plan,
                left,
                right,
            } => {
                let (a, b) = match plan {
                    ExtremumPlan::Left => return left.evaluate_cached(x, y, z, volume, memo),
                    ExtremumPlan::Right => return right.evaluate_cached(x, y, z, volume, memo),
                    ExtremumPlan::LeftConstant => (
                        right.evaluate_cached(x, y, z, volume, memo),
                        left.evaluate_cached(x, y, z, volume, memo),
                    ),
                    ExtremumPlan::RightConstant => (
                        left.evaluate_cached(x, y, z, volume, memo),
                        right.evaluate_cached(x, y, z, volume, memo),
                    ),
                    ExtremumPlan::General(bound) => {
                        let a = left.evaluate_cached(x, y, z, volume, memo);
                        if !volume && if *minimum { a <= *bound } else { a >= *bound } {
                            return a;
                        }
                        (a, right.evaluate_cached(x, y, z, volume, memo))
                    }
                };
                if *minimum {
                    interval::min(a, b)
                } else {
                    interval::max(a, b)
                }
            }
            Node::Unary { op, input } => {
                let a = input.evaluate_cached(x, y, z, volume, memo);
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
                let v = input.evaluate_cached(x, y, z, volume, memo);
                if v >= *min && v < *max {
                    inside.evaluate_cached(x, y, z, volume, memo)
                } else {
                    outside.evaluate_cached(x, y, z, volume, memo)
                }
            }
            Node::Lerp {
                alpha,
                first,
                second,
            } => {
                let alpha = alpha.evaluate_cached(x, y, z, volume, memo);
                if alpha == 0.0 {
                    return first.evaluate_cached(x, y, z, volume, memo);
                }
                if alpha == 1.0 {
                    return second.evaluate_cached(x, y, z, volume, memo);
                }
                lerp(
                    alpha,
                    first.evaluate_cached(x, y, z, volume, memo),
                    second.evaluate_cached(x, y, z, volume, memo),
                )
            }
            Node::Interpolated {
                input,
                cell_xz,
                cell_y,
            } => {
                // The Java compiler slices axes outside this function's domain
                // to zero before sampling, even when the result is non-finite.
                let x = if self.5 & 1 == 0 { 0 } else { x };
                let y = if self.5 & 2 == 0 { 0 } else { y };
                let z = if self.5 & 4 == 0 { 0 } else { z };
                if volume {
                    return interpolate_block_volume(
                        input,
                        *cell_xz,
                        *cell_y,
                        self.5,
                        [x, y, z],
                        memo,
                    );
                }
                let rx = x.rem_euclid(*cell_xz);
                let ry = y.rem_euclid(*cell_y);
                let rz = z.rem_euclid(*cell_xz);
                if rx == 0 && ry == 0 && rz == 0 {
                    return input.evaluate_cached(x, y, z, false, memo);
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
                let mut corner = |x, y, z| input.evaluate_cached(x, y, z, true, memo);
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
                    px += f64::from(shifts[0].evaluate_cached(x, y, z, volume, memo));
                    pz += f64::from(shifts[2].evaluate_cached(x, y, z, volume, memo));
                    if *shift_mode == 2 {
                        py += f64::from(shifts[1].evaluate_cached(x, y, z, volume, memo));
                    }
                }
                noise.sample(px, py, pz)
            }
            Node::Shift { noise, kind } => match (kind, volume) {
                (ShiftKind::A, false) => {
                    noise.sample(f64::from(x) * 0.25, 0.0, f64::from(z) * 0.25) * 4.0
                }
                (ShiftKind::A, true) => noise.sample_scaled_volume(x, 0, z, 0.25, 0.0) * 4.0,
                (ShiftKind::B, false) => {
                    noise.sample(f64::from(z) * 0.25, f64::from(x) * 0.25, 0.0) * 4.0
                }
                (ShiftKind::B, true) => {
                    noise.sample_scaled_volume_weighted(z, x, 0, 0.25, 0.25, 4.0)
                }
                (ShiftKind::ThreeDimensional, false) => {
                    noise.sample(
                        f64::from(x) * 0.25,
                        f64::from(y) * 0.25,
                        f64::from(z) * 0.25,
                    ) * 4.0
                }
                (ShiftKind::ThreeDimensional, true) => {
                    noise.sample_scaled_volume(x, y, z, 0.25, 0.25) * 4.0
                }
            },
            Node::EndOuterIslands(noise) => end_outer_island_density(noise, x, z),
            Node::OldBlendedNoise(noise) => noise.sample(x, y, z, volume),
            Node::IntervalSelect {
                input,
                thresholds,
                functions,
            } => {
                let value = input.evaluate_cached(x, y, z, volume, memo);
                let index = thresholds
                    .iter()
                    .position(|threshold| value < *threshold)
                    .unwrap_or(thresholds.len());
                functions[index].evaluate_cached(x, y, z, volume, memo)
            }
            Node::Spline(spline) => spline.sample(x, y, z, volume, memo),
            Node::UnblendedAlpha => 1.0,
            Node::UnblendedOffset => 0.0,
            Node::UnblendedDensity(input) => input.evaluate_cached(x, y, z, volume, memo),
            Node::NoStructures => 0.0,
        }
    }
}

fn lerp(alpha: f32, first: f32, second: f32) -> f32 {
    first + alpha * (second - first)
}

fn interpolate_block_volume(
    input: &Arc<DensityFunction>,
    cell_xz: i32,
    cell_y: i32,
    axes: u8,
    position: [i32; 3],
    memo: &mut SampleMemo,
) -> f32 {
    let mut bounds = memo.volume.expect("block volume context is required");
    for axis in 0..3 {
        if axes & (1 << axis) == 0 {
            bounds.min[axis] = 0;
            bounds.max[axis] = 0;
        }
    }
    let cell = [cell_xz, cell_y, cell_xz];
    let aligned = (0..3).all(|axis| {
        bounds.min[axis].rem_euclid(cell[axis]) == 0
            && (bounds.min[axis] == bounds.max[axis] || cell[axis] == 1)
    });
    if aligned {
        return input.evaluate_cached(position[0], position[1], position[2], true, memo);
    }
    let mut base = [0; 3];
    let mut next = [0; 3];
    let mut offset = [0; 3];
    for axis in 0..3 {
        offset[axis] = position[axis].rem_euclid(cell[axis]);
        base[axis] = position[axis].wrapping_sub(offset[axis]);
        next[axis] = if base[axis] == bounds.max[axis] {
            base[axis]
        } else {
            base[axis].wrapping_add(cell[axis])
        };
    }
    let mut corner = |x, y, z| {
        let key = memo_key(input, x, y, z, true);
        if let Some(value) = memo.get(&key) {
            return *value;
        }
        let value = input.evaluate_cached(x, y, z, true, memo);
        memo.insert(key, value);
        value
    };
    let c000 = corner(base[0], base[1], base[2]);
    let c100 = corner(next[0], base[1], base[2]);
    let c010 = corner(base[0], next[1], base[2]);
    let c110 = corner(next[0], next[1], base[2]);
    let c001 = corner(base[0], base[1], next[2]);
    let c101 = corner(next[0], base[1], next[2]);
    let c011 = corner(base[0], next[1], next[2]);
    let c111 = corner(next[0], next[1], next[2]);
    let tz = offset[2] as f32 * (1.0 / cell_xz as f32);
    let tx = offset[0] as f32 * (1.0 / cell_xz as f32);
    let bottom = lerp(tx, lerp(tz, c000, c001), lerp(tz, c100, c101));
    let top = lerp(tx, lerp(tz, c010, c011), lerp(tz, c110, c111));
    let increment = (top - bottom) * (1.0 / cell_y as f32);
    let start_y = bounds.min[1].wrapping_sub(base[1]).max(0);
    let mut value = bottom + increment * start_y as f32;
    for _ in start_y..offset[1] {
        value += increment;
    }
    value
}

fn end_outer_island_density(noise: &Simplex2D, x: i32, z: i32) -> f32 {
    let x = x / 8;
    let z = z / 8;
    let (cell_x, cell_z) = (x / 2, z / 2);
    let (local_x, local_z) = (x % 2, z % 2);
    let mut height = -100.0_f32;
    for dx in -12_i32..=12 {
        for dz in -12_i32..=12 {
            let px = i64::from(cell_x.wrapping_add(dx));
            let pz = i64::from(cell_z.wrapping_add(dz));
            if px * px + pz * pz <= 4096 {
                continue;
            }
            let ox = (local_x - dx * 2) as f32;
            let oz = (local_z - dz * 2) as f32;
            let distance = (ox * ox + oz * oz).sqrt();
            // The island scale is always at least nine. Skip simplex sampling
            // when even the largest possible candidate cannot improve height.
            if 100.0 - distance * 9.0 <= height || noise.sample(px as f64, pz as f64) >= -0.9 {
                continue;
            }
            let scale = ((px as f32).abs() * 3439.0 + (pz as f32).abs() * 147.0) % 13.0 + 9.0;
            let candidate = (100.0 - distance * scale).clamp(-100.0, 80.0);
            height = height.max(candidate);
        }
    }
    (height - 8.0) / 128.0
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
        Node::Constant(_)
        | Node::Gradient { .. }
        | Node::DistanceToPoint { .. }
        | Node::EndOuterIslands(_)
        | Node::OldBlendedNoise(_)
        | Node::Shift { .. }
        | Node::Spline(_)
        | Node::UnblendedAlpha
        | Node::UnblendedOffset => vec![],
        Node::NoStructures => vec![],
        Node::Reference(value)
        | Node::Cache(value)
        | Node::UnblendedDensity(value)
        | Node::Clamp { input: value, .. }
        | Node::Slice { input: value, .. }
        | Node::Unary { input: value, .. }
        | Node::Interpolated { input: value, .. } => vec![value],
        Node::Binary { left, right, .. } | Node::Extremum { left, right, .. } => vec![left, right],
        Node::FindTopSurface { density, upper, .. } => vec![density, upper],
        Node::Range {
            input,
            inside,
            outside,
            ..
        } => vec![input, inside, outside],
        Node::IntervalSelect {
            input, functions, ..
        } => std::iter::once(input).chain(functions.iter()).collect(),
        Node::Lerp {
            alpha,
            first,
            second,
        } => vec![alpha, first, second],
        Node::Noise { shifts, .. } => shifts.iter().collect(),
    };
    let mut depth = 1 + children.iter().map(|child| child.1).max().unwrap_or(0);
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
    let mut has_interpolation =
        matches!(node, Node::Interpolated { .. }) || children.iter().any(|child| child.3);
    let mut bulk_safe = children.iter().all(|child| child.6);
    let mut axes = children.iter().fold(0, |axes, child| axes | child.5);
    match &node {
        Node::Gradient { axis, .. } => axes = 1 << axis,
        Node::DistanceToPoint { .. } => axes = 7,
        Node::EndOuterIslands(_) => axes = 5,
        Node::OldBlendedNoise(_) => axes = 7,
        Node::Slice { axis, input, .. } => {
            axes &= !(1 << axis);
            bulk_safe &= !input.3;
        }
        Node::FindTopSurface { upper, .. } => {
            axes &= !2;
            has_interpolation = upper.3;
            bulk_safe = upper.6 && !upper.3;
        }
        Node::Noise {
            xz_scale, y_scale, ..
        } => {
            if *xz_scale != 0.0 {
                axes |= 5;
            }
            if *y_scale != 0.0 {
                axes |= 2;
            }
        }
        Node::Shift { kind, .. } => {
            axes = match kind {
                ShiftKind::A | ShiftKind::B => 5,
                ShiftKind::ThreeDimensional => 7,
            }
        }
        Node::UnblendedAlpha | Node::UnblendedOffset => axes = 5,
        Node::NoStructures => axes = 7,
        _ => {}
    }
    if let Node::Spline(spline) = &node {
        depth = 1 + spline.depth();
        work = 1usize.saturating_add(spline.work());
        has_interpolation = spline.has_interpolation();
        axes = spline.domain_axes();
        bulk_safe = spline.bulk_safe();
    }
    ensure!(depth <= MAX_DEPTH, "density function exceeds maximum depth");
    ensure!(
        work <= MAX_NODES,
        "density function exceeds maximum sampling work"
    );
    let range = match &node {
        Node::Constant(value) => Interval::exact(*value),
        Node::Reference(input)
        | Node::Cache(input)
        | Node::UnblendedDensity(input)
        | Node::Interpolated { input, .. } => input.4,
        Node::Slice { input, .. } => input.4,
        Node::FindTopSurface { lower, upper, .. } => {
            Interval::bounds(*lower as f32, interval::max(*lower as f32, upper.4.max))
        }
        Node::Clamp { input, min, max } => input.4.clamp(*min, *max),
        Node::Gradient {
            value, to_value, ..
        } => Interval::enclosing_values(*value, *to_value),
        Node::DistanceToPoint { .. } => Interval::bounds(0.0, f32::INFINITY),
        Node::EndOuterIslands(_) => Interval::bounds(-0.84375, 0.5625),
        Node::OldBlendedNoise(noise) => {
            let (min, max) = noise.range();
            Interval::bounds(min, max)
        }
        Node::Binary { op, left, right } => match op {
            Binary::Add => left.4.add(right.4),
            Binary::Sub => left.4.sub(right.4),
            Binary::Mul => left.4.mul(right.4),
            Binary::Div => left.4.div(right.4),
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
        Node::IntervalSelect { functions, .. } => functions
            .iter()
            .fold(Interval::NAI, |range, f| Interval::enclosing(range, f.4)),
        Node::Lerp {
            alpha,
            first,
            second,
        } => alpha.4.lerp(first.4, second.4),
        Node::Noise { noise, .. } => {
            let (min, max) = noise.range();
            Interval::bounds(min, max)
        }
        Node::Shift { noise, .. } => {
            let (min, max) = noise.range();
            Interval::bounds(min, max).mul(Interval::exact(4.0))
        }
        Node::Spline(spline) => spline.range(),
        Node::UnblendedAlpha => Interval::bounds(0.0, 1.0),
        Node::UnblendedOffset => Interval::bounds(f32::NEG_INFINITY, f32::INFINITY),
        Node::NoStructures => Interval::bounds(f32::NEG_INFINITY, f32::INFINITY),
    };
    Ok(Arc::new(DensityFunction(
        node,
        depth,
        work,
        has_interpolation,
        range,
        axes,
        bulk_safe,
    )))
}

/// Resolves functions from the active data-pack stack, including its overrides.
pub struct DensityCompiler<'a, P: ResourceProvider> {
    resources: &'a mut P,
    cache: BTreeMap<ResourceId, Arc<DensityFunction>>,
    cache_nodes: BTreeMap<String, Arc<DensityFunction>>,
    spline_coordinates: BTreeMap<String, Arc<DensityFunction>>,
    active: BTreeSet<ResourceId>,
    nodes: usize,
    noise_factory: Option<WorldNoiseFactory>,
    world_seed: Option<i64>,
    noises: BTreeMap<ResourceId, Arc<NormalNoise>>,
    unblended: bool,
    no_structure_adjustment: bool,
    end_island_noise: Option<Arc<Simplex2D>>,
    blended_noises: BTreeMap<String, Arc<BlendedNoise>>,
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
            cache_nodes: BTreeMap::new(),
            spline_coordinates: BTreeMap::new(),
            active: BTreeSet::new(),
            nodes: 0,
            noise_factory: None,
            world_seed: None,
            noises: BTreeMap::new(),
            unblended: false,
            no_structure_adjustment: false,
            end_island_noise: None,
            blended_noises: BTreeMap::new(),
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

    /// Use only where no older chunk data needs terrain blending.
    pub fn without_old_chunk_blending(mut self) -> Self {
        self.unblended = true;
        self
    }

    /// Use only when no structures affect the sampled terrain.
    pub fn without_structure_terrain_adjustment(mut self) -> Self {
        self.no_structure_adjustment = true;
        self
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
        if id.path == "cache" {
            let input = field("input")?;
            let key = serde_json::to_string(input)?;
            if let Some(function) = self.cache_nodes.get(&key) {
                return Ok(Arc::clone(function));
            }
            let child = self.expression(input, depth + 1)?;
            let function = make(Node::Cache(child))?;
            self.cache_nodes.insert(key, Arc::clone(&function));
            return Ok(function);
        }
        let node = match id.path.as_str() {
            "constant" => Node::Constant(number(field("value")?)?),
            "blend_alpha" | "blend_offset" => {
                ensure!(self.unblended, "terrain blending context is required");
                if id.path == "blend_alpha" {
                    Node::UnblendedAlpha
                } else {
                    Node::UnblendedOffset
                }
            }
            "blend_density" => {
                ensure!(self.unblended, "terrain blending context is required");
                Node::UnblendedDensity(self.expression(field("input")?, depth + 1)?)
            }
            "beardifier" => {
                ensure!(
                    self.no_structure_adjustment,
                    "structure terrain adjustment context is required"
                );
                Node::NoStructures
            }
            "spline" => Node::Spline(Spline::parse(self, field("spline")?, depth + 1)?),
            "old_blended_noise" => {
                let parse = |name: &str, min: f64, max: f64| -> Result<f64> {
                    let value = field(name)?
                        .as_f64()
                        .with_context(|| format!("{name} must be a number"))?;
                    ensure!(
                        value.is_finite() && (min..=max).contains(&value),
                        "{name} is out of range"
                    );
                    Ok(value)
                };
                let params = [
                    parse("xz_scale", 0.001, 1000.0)?,
                    parse("y_scale", 0.001, 1000.0)?,
                    parse("xz_factor", 0.001, 1000.0)?,
                    parse("y_factor", 0.001, 1000.0)?,
                    parse("smear_scale_multiplier", 1.0, 8.0)?,
                ];
                let key = serde_json::to_string(&params)?;
                let noise = if let Some(noise) = self.blended_noises.get(&key) {
                    Arc::clone(noise)
                } else {
                    let factory = self
                        .noise_factory
                        .as_ref()
                        .context("a world seed is required for blended terrain noise")?;
                    let make_noise = |rng: &mut _| {
                        BlendedNoise::new(
                            rng, params[0], params[1], params[2], params[3], params[4],
                        )
                    };
                    let noise = Arc::new(match factory {
                        WorldNoiseFactory::Legacy(_) => {
                            make_noise(&mut LegacyRandom::new(self.world_seed.unwrap()))
                        }
                        WorldNoiseFactory::Xoroshiro(factory) => BlendedNoise::new(
                            &mut factory.random_for("minecraft:terrain"),
                            params[0],
                            params[1],
                            params[2],
                            params[3],
                            params[4],
                        ),
                    });
                    self.blended_noises.insert(key, Arc::clone(&noise));
                    noise
                };
                Node::OldBlendedNoise(noise)
            }
            "end_outer_islands" => {
                let seed = self
                    .world_seed
                    .context("end island noise requires a world seed")?;
                let noise = self.end_island_noise.get_or_insert_with(|| {
                    let mut random = LegacyRandom::new(seed);
                    random.skip_ints(17_292);
                    Arc::new(Simplex2D::from_random(&mut random, true))
                });
                Node::EndOuterIslands(Arc::clone(noise))
            }
            "distance_to_point" => {
                let point = field("point")?
                    .as_array()
                    .context("distance point must be an array")?;
                ensure!(
                    point.len() == 3,
                    "distance point must contain three coordinates"
                );
                let metric = match field("metric")?.as_str() {
                    Some("euclidean") => DistanceMetric::Euclidean,
                    Some("euclidean_squared") => DistanceMetric::EuclideanSquared,
                    Some("manhattan") => DistanceMetric::Manhattan,
                    Some("chebyshev") => DistanceMetric::Chebyshev,
                    _ => bail!("invalid distance metric"),
                };
                Node::DistanceToPoint {
                    point: [
                        integer(&point[0])?,
                        integer(&point[1])?,
                        integer(&point[2])?,
                    ],
                    metric,
                }
            }
            "slice" => {
                let axis = match field("axis")?.as_str() {
                    Some("x") => 0,
                    Some("y") => 1,
                    Some("z") => 2,
                    _ => bail!("invalid slice axis"),
                };
                Node::Slice {
                    axis,
                    coordinate: integer(field("coordinate")?)?,
                    input: self.expression(field("input")?, depth + 1)?,
                }
            }
            "find_top_surface" => {
                let lower = integer(field("lower_bound")?)?;
                ensure!(
                    (-4064..=4062).contains(&lower),
                    "surface lower bound is out of range"
                );
                let cell_height = integer(field("cell_height")?)?;
                ensure!(cell_height > 0, "surface cell height must be positive");
                let density = self.expression(field("density")?, depth + 1)?;
                let upper = self.expression(field("upper_bound")?, depth + 1)?;
                let max = f64::from(upper.4.max);
                ensure!(
                    upper.4.min.is_finite()
                        && upper.4.min >= -1_000_000_000.0
                        && max.is_finite()
                        && max <= 1_000_000_000.0
                        && (max - f64::from(lower)) / f64::from(cell_height) <= MAX_SURFACE_STEPS,
                    "surface search exceeds supported sampling work"
                );
                Node::FindTopSurface {
                    density,
                    upper,
                    lower,
                    cell_height,
                }
            }
            "shift_a" | "shift_b" | "shift" => {
                let name = field("noise")?
                    .as_str()
                    .context("shift noise must reference a registered noise definition")?;
                let noise = self.noise(name)?;
                let kind = match id.path.as_str() {
                    "shift_a" => ShiftKind::A,
                    "shift_b" => ShiftKind::B,
                    _ => ShiftKind::ThreeDimensional,
                };
                Node::Shift { noise, kind }
            }
            "clamp" => {
                let min = number(field("min")?)?;
                let max = number(field("max")?)?;
                ensure!(min <= max, "clamp minimum exceeds maximum");
                Node::Clamp {
                    input: self.expression(field("input")?, depth + 1)?,
                    min,
                    max,
                }
            }
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
            "add" | "sub" | "mul" | "div" => {
                let op = match id.path.as_str() {
                    "add" => Binary::Add,
                    "sub" => Binary::Sub,
                    "mul" => Binary::Mul,
                    "div" => Binary::Div,
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
            "interval_select" => {
                let thresholds: Vec<f32> = field("thresholds")?
                    .as_array()
                    .context("thresholds must be an array")?
                    .iter()
                    .map(number)
                    .collect::<Result<_>>()?;
                ensure!(
                    thresholds
                        .windows(2)
                        .all(|pair| pair[0].total_cmp(&pair[1]).is_le()),
                    "thresholds must be ordered"
                );
                let values = field("functions")?
                    .as_array()
                    .context("functions must be an array")?;
                ensure!(
                    values.len() >= 2 && values.len() == thresholds.len() + 1,
                    "interval selection requires one more function than thresholds"
                );
                let input = self.expression(field("input")?, depth + 1)?;
                let functions = values
                    .iter()
                    .map(|value| self.expression(value, depth + 1))
                    .collect::<Result<_>>()?;
                Node::IntervalSelect {
                    input,
                    thresholds,
                    functions,
                }
            }
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
        data.insert("noise", json!({"type":"minecraft:unknown_noise"}));
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

    #[test]
    fn cache_nodes_share_evaluation_and_keep_volume_keys_separate() {
        let mut data = Data::default();
        let mut compiler = DensityCompiler::new(&mut data);
        let cached = json!({"type":"cache","input":{"type":"gradient","axis":"y","from_coordinate":0,"to_coordinate":10,"from_value":0,"to_value":1}});
        let function = compiler
            .compile(&json!({"type":"add","left":cached,"right":cached}))
            .unwrap();
        let Node::Binary { left, right, .. } = &function.0 else {
            panic!("expected binary");
        };
        assert!(Arc::ptr_eq(left, right));
        let mut memo = SampleMemo::default();
        let value = function.evaluate_cached(1, 5, 3, false, &mut memo);
        assert_eq!(value, 1.0);
        assert_eq!(memo.len(), 1);
        assert_eq!(
            function.evaluate_cached(100, 5, -100, false, &mut memo),
            1.0
        );
        assert_eq!(memo.len(), 1);
        function.evaluate_cached(1, 5, 3, true, &mut memo);
        assert_eq!(memo.len(), 2);
        function.evaluate_cached(1, 6, 3, false, &mut memo);
        assert_eq!(memo.len(), 3);
        let mut evaluator = DensityEvaluator::default();
        assert_eq!(evaluator.sample(&function, 1, 5, 3), 1.0);
        let capacity = evaluator.memo.capacity();
        assert_eq!(evaluator.sample(&function, 1, 6, 3), 1.2);
        assert!(evaluator.memo.capacity() >= capacity);
        assert!(
            compiler
                .compile(&json!({"type":"clamp","input":0,"min":1,"max":-1}))
                .is_err()
        );
    }

    #[test]
    fn shift_functions_share_noise_and_horizontal_shifts_ignore_height() {
        let mut data = Data::default();
        data.0.insert(
            "data/test/worldgen/noise/offset.json".to_owned(),
            br#"{"base_octave":-3}"#.to_vec(),
        );
        let mut compiler =
            DensityCompiler::with_world_seed(&mut data, 47, WorldgenRandom::Xoroshiro);
        let a = compiler
            .compile(&json!({"type":"shift_a","noise":"test:offset"}))
            .unwrap();
        let b = compiler
            .compile(&json!({"type":"shift_b","noise":"test:offset"}))
            .unwrap();
        let c = compiler
            .compile(&json!({"type":"shift","noise":"test:offset"}))
            .unwrap();
        let (
            Node::Shift { noise: first, .. },
            Node::Shift { noise: second, .. },
            Node::Shift { noise: third, .. },
        ) = (&a.0, &b.0, &c.0)
        else {
            panic!("expected shift functions");
        };
        assert!(Arc::ptr_eq(first, second) && Arc::ptr_eq(first, third));
        assert_eq!(
            a.sample(12, -100, 23).to_bits(),
            a.sample(12, 100, 23).to_bits()
        );
        assert_eq!(
            b.sample(12, -100, 23).to_bits(),
            b.sample(12, 100, 23).to_bits()
        );
    }

    #[test]
    fn spline_validates_structure_and_interns_equal_coordinates() {
        let mut data = Data::default();
        let mut compiler = DensityCompiler::new(&mut data);
        assert!(
            compiler
                .compile(&json!({"type":"spline","spline":{"coordinate":0,"points":[]}}))
                .is_err()
        );
        let previous_coordinates = compiler.spline_coordinates.len();
        let coordinate = json!({"type":"gradient","axis":"y","from_coordinate":0,"to_coordinate":8,"from_value":0,"to_value":1});
        let inner =
            json!({"coordinate":coordinate,"points":[{"location":0,"value":2,"derivative":0}]});
        let value = json!({"type":"spline","spline":{"coordinate":coordinate,"points":[{"location":0,"value":inner,"derivative":0},{"location":1,"value":3,"derivative":0}]}});
        let compiled = compiler.compile(&value).unwrap();
        let mut evaluator = DensityEvaluator::default();
        assert_eq!(evaluator.sample(&compiled, 0, 0, 0), 2.0);
        assert_eq!(compiler.spline_coordinates.len(), previous_coordinates + 1);
        let large = compiler
            .compile(&json!({"type":"spline","spline":123456789.0}))
            .unwrap();
        assert_eq!(large.sample(0, 0, 0), 123456789.0_f32);
    }

    #[test]
    fn unblended_defaults_require_explicit_context_and_keep_java_ranges() {
        let mut data = Data::default();
        assert!(
            DensityCompiler::new(&mut data)
                .compile(&json!({"type":"blend_alpha"}))
                .is_err()
        );
        let mut compiler = DensityCompiler::new(&mut data).without_old_chunk_blending();
        let alpha = compiler.compile(&json!({"type":"blend_alpha"})).unwrap();
        let offset = compiler.compile(&json!({"type":"blend_offset"})).unwrap();
        assert_eq!(alpha.sample(1, 2, 3), 1.0);
        assert_eq!(alpha.range(), (0.0, 1.0));
        assert_eq!(offset.sample(1, 2, 3), 0.0);
        assert_eq!(offset.range(), (f32::NEG_INFINITY, f32::INFINITY));
        assert!(compiler.compile(&json!({"type":"beardifier"})).is_err());
        let blended = compiler
            .compile(&json!({"type":"blend_density","input":0.25}))
            .unwrap();
        assert_eq!(blended.sample(1, 2, 3), 0.25);
        assert_eq!(blended.range(), (0.25, 0.25));
        let mut compiler = DensityCompiler::new(&mut data).without_structure_terrain_adjustment();
        assert!(
            compiler
                .compile(&json!({"type":"blend_density","input":0}))
                .is_err()
        );
        let beardifier = compiler.compile(&json!({"type":"beardifier"})).unwrap();
        assert_eq!(beardifier.sample(1, 2, 3), 0.0);
        assert_eq!(beardifier.range(), (f32::NEG_INFINITY, f32::INFINITY));
    }

    #[test]
    fn interval_selection_validates_thresholds_and_selects_equal_upper_interval() {
        let mut data = Data::default();
        let mut compiler = DensityCompiler::new(&mut data);
        for value in [
            json!({"type":"interval_select","input":0,"thresholds":[1,0],"functions":[0,1,2]}),
            json!({"type":"interval_select","input":0,"thresholds":[0],"functions":[0]}),
            json!({"type":"interval_select","input":0,"thresholds":[],"functions":[0]}),
        ] {
            assert!(compiler.compile(&value).is_err());
        }
        let select = compiler
            .compile(
                &json!({"type":"interval_select","input":0,"thresholds":[0,0],"functions":[1,2,3]}),
            )
            .unwrap();
        assert_eq!(select.sample(0, 0, 0), 3.0);
        assert_eq!(select.range(), (1.0, 3.0));
    }

    #[test]
    fn distance_and_slice_reject_invalid_data_pack_definitions() {
        let mut data = Data::default();
        let mut compiler = DensityCompiler::new(&mut data);
        for value in [
            json!({"type":"distance_to_point","point":[0,0],"metric":"euclidean"}),
            json!({"type":"distance_to_point","point":[0,0,0],"metric":"unknown"}),
            json!({"type":"slice","axis":"w","coordinate":0,"input":0}),
            json!({"type":"slice","axis":"x","coordinate":2147483648_i64,"input":0}),
        ] {
            assert!(compiler.compile(&value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn end_islands_require_a_seed_and_share_the_initialized_noise() {
        let mut data = Data::default();
        let expression = json!({"type":"end_outer_islands"});
        assert!(
            DensityCompiler::new(&mut data)
                .compile(&expression)
                .is_err()
        );
        let mut compiler =
            DensityCompiler::with_world_seed(&mut data, 123, WorldgenRandom::Xoroshiro);
        let first = compiler.compile(&expression).unwrap();
        let second = compiler.compile(&expression).unwrap();
        let (Node::EndOuterIslands(first), Node::EndOuterIslands(second)) = (&first.0, &second.0)
        else {
            panic!("expected end island samplers");
        };
        assert!(Arc::ptr_eq(first, second));
    }

    #[test]
    fn blended_noise_checks_parameters_and_shares_identical_graphs() {
        let mut data = Data::default();
        let expression = json!({"type":"old_blended_noise","xz_scale":0.25,"y_scale":0.125,"xz_factor":80.0,"y_factor":160.0,"smear_scale_multiplier":8.0});
        assert!(
            DensityCompiler::new(&mut data)
                .compile(&expression)
                .is_err()
        );
        let mut compiler =
            DensityCompiler::with_world_seed(&mut data, 123, WorldgenRandom::Xoroshiro);
        let first = compiler.compile(&expression).unwrap();
        let second = compiler.compile(&expression).unwrap();
        let (Node::OldBlendedNoise(first), Node::OldBlendedNoise(second)) = (&first.0, &second.0)
        else {
            panic!("expected blended noise");
        };
        assert!(Arc::ptr_eq(first, second));
        let mut invalid = expression;
        invalid["y_factor"] = json!(0.0);
        assert!(compiler.compile(&invalid).is_err());
    }

    #[test]
    fn surface_search_scans_inclusive_cells_and_ignores_input_height() {
        let mut data = Data::default();
        let mut compiler = DensityCompiler::new(&mut data);
        let surface = json!({"type":"find_top_surface", "density":{"type":"gradient","axis":"y","from_coordinate":0,"to_coordinate":16,"from_value":1,"to_value":-1},"upper_bound":{"type":"gradient","axis":"y","from_coordinate":0,"to_coordinate":16,"from_value":32,"to_value":96},"lower_bound":-16,"cell_height":8});
        let function = compiler.compile(&surface).unwrap();
        assert_eq!(function.sample(0, -500, 0), 0.0);
        assert_eq!(function.sample(0, 500, 0), 0.0);
        assert_eq!(function.range(), (-16.0, 96.0));
        let mut invalid = surface;
        invalid["cell_height"] = json!(0);
        assert!(compiler.compile(&invalid).is_err());
        invalid["cell_height"] = json!(1);
        invalid["upper_bound"] = json!({"type":"mul","left":1000000,"right":1000000});
        assert!(compiler.compile(&invalid).is_err());
    }

    #[test]
    fn block_volume_rejects_unsupported_or_invalid_shapes() {
        let mut data = Data::default();
        let mut compiler = DensityCompiler::new(&mut data);
        let value = compiler.compile(&json!({"type":"slice","axis":"y","coordinate":3,"input":{"type":"interpolated","cell_size_xz":4,"cell_size_y":8,"input":1}})).unwrap();
        let mut evaluator = DensityEvaluator::default();
        assert!(
            evaluator
                .sample_block_volume(&value, [0, 0, 0], [2, 2, 2])
                .is_err()
        );
        let constant = compiler.compile(&json!(1)).unwrap();
        assert!(
            evaluator
                .sample_block_volume(&constant, [0, 0, 0], [0, 1, 1])
                .is_err()
        );
        assert!(
            evaluator
                .sample_block_volume(&constant, [i32::MAX, 0, 0], [2, 1, 1])
                .is_err()
        );
        assert_eq!(
            evaluator
                .sample_block_volume(&constant, [0, 0, 0], [2, 2, 2])
                .unwrap(),
            vec![1.0; 8]
        );
    }
}
