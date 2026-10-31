//! Java 26.3's nested cubic spline density functions.
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::{
    DensityCompiler, DensityFunction, MAX_DEPTH, MAX_NODES, SampleMemo,
    interval::{self, Interval},
    lerp, memo_key,
};
use crate::packs::ResourceProvider;

#[derive(Debug)]
pub(super) enum Spline {
    Constant(f32),
    Multipoint {
        coordinate: Arc<DensityFunction>,
        points: Vec<Point>,
        range: Interval,
        depth: usize,
        work: usize,
        interpolation: bool,
    },
}

#[derive(Debug)]
pub(super) struct Point {
    location: f32,
    value: Spline,
    derivative: f32,
}

fn float(value: &Value) -> Result<f32> {
    Ok(value.as_f64().context("spline value must be a number")? as f32)
}

impl Spline {
    pub fn parse<P: ResourceProvider>(
        compiler: &mut DensityCompiler<'_, P>,
        value: &Value,
        depth: usize,
    ) -> Result<Self> {
        ensure!(depth <= MAX_DEPTH, "spline exceeds maximum depth");
        compiler.nodes += 1;
        ensure!(
            compiler.nodes <= MAX_NODES,
            "density function exceeds maximum node count"
        );
        if value.is_number() {
            return Ok(Self::Constant(float(value)?));
        }
        let object = value
            .as_object()
            .context("spline must be a number or object")?;
        let field = |name: &str| {
            object
                .get(name)
                .with_context(|| format!("missing spline field {name}"))
        };
        let coordinate_value = field("coordinate")?;
        let key = serde_json::to_string(coordinate_value)?;
        let coordinate = if let Some(existing) = compiler.spline_coordinates.get(&key) {
            Arc::clone(existing)
        } else {
            let compiled = compiler.expression(coordinate_value, depth + 1)?;
            compiler
                .spline_coordinates
                .insert(key, Arc::clone(&compiled));
            compiled
        };
        let entries = field("points")?
            .as_array()
            .context("spline points must be an array")?;
        ensure!(!entries.is_empty(), "spline must have at least one point");
        ensure!(
            entries.len() <= MAX_NODES.saturating_sub(compiler.nodes),
            "spline exceeds maximum node count"
        );
        let mut points = Vec::with_capacity(entries.len());
        let mut spline_depth = 1 + coordinate.1;
        let mut work = 1usize.saturating_add(coordinate.2);
        let mut interpolation = coordinate.3;
        for entry in entries {
            let entry = entry
                .as_object()
                .context("spline point must be an object")?;
            let get = |name: &str| {
                entry
                    .get(name)
                    .with_context(|| format!("missing spline point field {name}"))
            };
            let location = float(get("location")?)?;
            let derivative = float(get("derivative")?)?;
            let value = Self::parse(compiler, get("value")?, depth + 1)?;
            spline_depth = spline_depth.max(1 + value.depth());
            work = work.saturating_add(value.work());
            interpolation |= value.has_interpolation();
            points.push(Point {
                location,
                value,
                derivative,
            });
        }
        ensure!(spline_depth <= MAX_DEPTH, "spline exceeds maximum depth");
        ensure!(work <= MAX_NODES, "spline exceeds maximum sampling work");
        let range = Self::calculate_range(coordinate.4, &points)?;
        Ok(Self::Multipoint {
            coordinate,
            points,
            range,
            depth: spline_depth,
            work,
            interpolation,
        })
    }

    pub fn depth(&self) -> usize {
        match self {
            Self::Constant(_) => 1,
            Self::Multipoint { depth, .. } => *depth,
        }
    }
    pub fn work(&self) -> usize {
        match self {
            Self::Constant(_) => 1,
            Self::Multipoint { work, .. } => *work,
        }
    }
    pub fn has_interpolation(&self) -> bool {
        match self {
            Self::Constant(_) => false,
            Self::Multipoint { interpolation, .. } => *interpolation,
        }
    }
    pub fn domain_axes(&self) -> u8 {
        match self {
            Self::Constant(_) => 0,
            Self::Multipoint {
                coordinate, points, ..
            } => points
                .iter()
                .fold(coordinate.5, |axes, point| axes | point.value.domain_axes()),
        }
    }
    pub fn bulk_safe(&self) -> bool {
        match self {
            Self::Constant(_) => true,
            Self::Multipoint {
                coordinate, points, ..
            } => coordinate.6 && points.iter().all(|point| point.value.bulk_safe()),
        }
    }
    pub fn range(&self) -> Interval {
        match self {
            Self::Constant(value) => Interval::exact(*value),
            Self::Multipoint { range, .. } => *range,
        }
    }

    pub fn sample(&self, x: i32, y: i32, z: i32, volume: bool, memo: &mut SampleMemo) -> f32 {
        let Self::Multipoint {
            coordinate, points, ..
        } = self
        else {
            if let Self::Constant(value) = self {
                return *value;
            }
            unreachable!()
        };
        let key = memo_key(coordinate, x, y, z, volume);
        let position = if let Some(value) = memo.get(&key) {
            *value
        } else {
            let value = coordinate.evaluate_cached(x, y, z, volume, memo);
            memo.insert(key, value);
            value
        };
        // Java's predicate is `position < location`, including for NaN.
        let mut lo = 0usize;
        let mut hi = points.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if position < points[mid].location {
                hi = mid
            } else {
                lo = mid + 1
            }
        }
        if lo == 0 {
            let point = &points[0];
            return extend(
                position,
                point.location,
                point.value.sample(x, y, z, volume, memo),
                point.derivative,
            );
        }
        if lo == points.len() {
            let point = &points[points.len() - 1];
            return extend(
                position,
                point.location,
                point.value.sample(x, y, z, volume, memo),
                point.derivative,
            );
        }
        let left = &points[lo - 1];
        let right = &points[lo];
        let width = right.location - left.location;
        let t = (position - left.location) / width;
        let first = left.value.sample(x, y, z, volume, memo);
        let second = right.value.sample(x, y, z, volume, memo);
        let a = left.derivative * width - (second - first);
        let b = -right.derivative * width + (second - first);
        lerp(t, first, second) + t * (1.0 - t) * lerp(t, a, b)
    }

    fn calculate_range(coordinate: Interval, points: &[Point]) -> Result<Interval> {
        if coordinate.min.is_nan() {
            return Ok(Interval::NAI);
        }
        let last = points.len() - 1;
        let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
        if coordinate.min < points[0].location {
            let p = &points[0];
            let r = p.value.range();
            let a = extend(coordinate.min, p.location, r.min, p.derivative);
            let b = extend(coordinate.min, p.location, r.max, p.derivative);
            min = interval::min(min, interval::min(a, b));
            max = interval::max(max, interval::max(a, b));
        }
        if coordinate.max > points[last].location {
            let p = &points[last];
            let r = p.value.range();
            let a = extend(coordinate.max, p.location, r.min, p.derivative);
            let b = extend(coordinate.max, p.location, r.max, p.derivative);
            min = interval::min(min, interval::min(a, b));
            max = interval::max(max, interval::max(a, b));
        }
        for point in points {
            let range = point.value.range();
            min = interval::min(min, range.min);
            max = interval::max(max, range.max);
        }
        for pair in points.windows(2) {
            let a = &pair[0];
            let b = &pair[1];
            if a.derivative == 0.0 && b.derivative == 0.0 {
                continue;
            }
            let width = b.location - a.location;
            let ar = a.value.range();
            let br = b.value.range();
            let d0 = a.derivative * width;
            let d1 = b.derivative * width;
            let low = interval::min(ar.min, br.min);
            let high = interval::max(ar.max, br.max);
            let p = d0 - br.max + ar.min;
            let q = d0 - br.min + ar.max;
            let r = -d1 + br.min - ar.max;
            let s = -d1 + br.max - ar.min;
            min = interval::min(min, low + 0.25 * interval::min(p, r));
            max = interval::max(max, high + 0.25 * interval::max(q, s));
        }
        ensure!(
            !min.is_nan() && !max.is_nan() && min <= max,
            "invalid spline output range"
        );
        Ok(Interval::bounds(min, max))
    }
}

fn extend(position: f32, location: f32, value: f32, derivative: f32) -> f32 {
    if derivative == 0.0 {
        value
    } else {
        value + derivative * (position - location)
    }
}
