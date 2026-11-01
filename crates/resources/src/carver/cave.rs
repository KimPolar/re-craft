//! Data-driven 26.3 cave carver. The resulting mask still needs the aquifer
//! and replaceable-block checks before it can be applied to a chunk.
use std::sync::OnceLock;

use anyhow::{Context, Result, bail, ensure};
use native_core::random::LegacyRandom;
use serde_json::Value;

use super::{CarvingMask, can_reach, carve_ellipsoid, source_random};
use crate::packs::{ResourceId, ResourceProvider};

pub(super) fn field<'a>(v: &'a Value, name: &str) -> Result<&'a Value> {
    v.get(name).with_context(|| format!("missing {name}"))
}
pub(super) fn int(v: &Value) -> Result<i32> {
    Ok(i32::try_from(v.as_i64().context("expected integer")?)?)
}
pub(super) fn float(v: &Value) -> Result<f32> {
    let n = v.as_f64().context("expected number")? as f32;
    ensure!(n.is_finite(), "non-finite number");
    Ok(n)
}
pub(super) fn kind(v: &Value) -> Result<&str> {
    let name = field(v, "type")?
        .as_str()
        .context("expected provider type")?;
    Ok(name.strip_prefix("minecraft:").unwrap_or(name))
}

#[derive(Clone, Debug)]
pub(super) enum FloatProvider {
    Constant(f32),
    Uniform(f32, f32),
    Trapezoid(f32, f32, f32),
}
impl FloatProvider {
    pub(super) fn parse(v: &Value) -> Result<Self> {
        if v.is_number() {
            return Ok(Self::Constant(float(v)?));
        }
        Ok(match kind(v)? {
            "constant" => Self::Constant(float(field(v, "value")?)?),
            "uniform" => {
                let (a, b) = (
                    float(field(v, "min_inclusive")?)?,
                    float(field(v, "max_exclusive")?)?,
                );
                ensure!(a < b, "invalid uniform float range");
                Self::Uniform(a, b)
            }
            "trapezoid" => {
                let (a, b, p) = (
                    float(field(v, "min")?)?,
                    float(field(v, "max")?)?,
                    float(field(v, "plateau")?)?,
                );
                ensure!(a <= b && p >= 0.0 && p <= b - a, "invalid trapezoid range");
                Self::Trapezoid(a, b, p)
            }
            name => bail!("unsupported float provider {name}"),
        })
    }
    pub(super) fn sample(&self, r: &mut LegacyRandom) -> f32 {
        match *self {
            Self::Constant(v) => v,
            Self::Uniform(a, b) => a + r.next_float() * (b - a),
            Self::Trapezoid(a, b, p) => {
                let left = (b - a - p) / 2.0;
                let right = b - a - left;
                a + r.next_float() * right + r.next_float() * left
            }
        }
    }
    fn range(&self) -> (f32, f32) {
        match *self {
            Self::Constant(n) => (n, n),
            Self::Uniform(a, b) | Self::Trapezoid(a, b, _) => (a, b),
        }
    }
}
#[derive(Clone, Debug)]
enum IntProvider {
    Constant(i32),
    Uniform(i32, i32),
    Biased(i32, i32),
    VeryBiased(i32, i32),
}
impl IntProvider {
    fn parse(v: &Value) -> Result<Self> {
        if v.is_number() {
            let n = int(v)?;
            ensure!(n >= 0, "negative count");
            return Ok(Self::Constant(n));
        }
        if kind(v)? == "constant" {
            return Self::parse(field(v, "value")?);
        }
        let a = int(field(v, "min_inclusive")?)?;
        let b = int(field(v, "max_inclusive")?)?;
        ensure!(
            a >= 0 && b >= a && i64::from(b) - i64::from(a) < i64::from(i32::MAX),
            "invalid count range"
        );
        Ok(match kind(v)? {
            "uniform" => Self::Uniform(a, b),
            "biased_to_bottom" => Self::Biased(a, b),
            "very_biased_to_bottom" => Self::VeryBiased(a, b),
            name => bail!("unsupported integer provider {name}"),
        })
    }
    fn sample(&self, r: &mut LegacyRandom) -> i32 {
        match *self {
            Self::Constant(v) => v,
            Self::Uniform(a, b) => a + r.next_int_bound(b - a + 1).unwrap(),
            Self::Biased(a, b) => {
                let n = r.next_int_bound(b - a + 1).unwrap();
                a + r.next_int_bound(n + 1).unwrap()
            }
            Self::VeryBiased(a, b) => {
                let n = r.next_int_bound(b - a + 1).unwrap();
                let n = r.next_int_bound(n + 1).unwrap();
                a + r.next_int_bound(n + 1).unwrap()
            }
        }
    }
}
#[derive(Clone, Debug)]
pub(super) enum Anchor {
    Absolute(i32),
    Above(i32),
    Below(i32),
}
impl Anchor {
    fn parse(v: &Value) -> Result<Self> {
        let o = v.as_object().context("expected vertical anchor")?;
        ensure!(o.len() == 1, "invalid vertical anchor");
        let (k, v) = o.iter().next().unwrap();
        let n = int(v)?;
        Ok(match k.as_str() {
            "absolute" => Self::Absolute(n),
            "above_bottom" => Self::Above(n),
            "below_top" => Self::Below(n),
            _ => bail!("unsupported vertical anchor {k}"),
        })
    }
    fn resolve(&self, min: i32, height: i32) -> i32 {
        match *self {
            Self::Absolute(n) => n,
            Self::Above(n) => min.wrapping_add(n),
            Self::Below(n) => min.wrapping_add(height).wrapping_sub(1).wrapping_sub(n),
        }
    }
}
#[derive(Clone, Debug)]
pub(super) enum HeightProvider {
    Constant(Anchor),
    Uniform(Anchor, Anchor),
}
impl HeightProvider {
    pub(super) fn parse(v: &Value) -> Result<Self> {
        if v.get("type").is_none() {
            return Ok(Self::Constant(Anchor::parse(v)?));
        }
        Ok(match kind(v)? {
            "constant" => Self::Constant(Anchor::parse(field(v, "value")?)?),
            "uniform" => Self::Uniform(
                Anchor::parse(field(v, "min_inclusive")?)?,
                Anchor::parse(field(v, "max_inclusive")?)?,
            ),
            name => bail!("unsupported height provider {name}"),
        })
    }
    pub(super) fn sample(&self, r: &mut LegacyRandom, min: i32, height: i32) -> Result<i32> {
        Ok(match self {
            Self::Constant(a) => a.resolve(min, height),
            Self::Uniform(a, b) => {
                let a = a.resolve(min, height);
                let b = b.resolve(min, height);
                if a > b {
                    a
                } else {
                    let width = i64::from(b) - i64::from(a) + 1;
                    ensure!(width <= i64::from(i32::MAX), "height range too large");
                    a + r.next_int_bound(width as i32).unwrap()
                }
            }
        })
    }
}

/// Cave settings from the active data-pack stack.
#[derive(Clone, Debug)]
pub struct CaveCarver {
    probability: f32,
    y: HeightProvider,
    count: IntProvider,
    thickness: FloatProvider,
    weird: bool,
    room: FloatProvider,
    horizontal: FloatProvider,
    vertical: FloatProvider,
    start_vertical: FloatProvider,
    floor: FloatProvider,
}
impl CaveCarver {
    pub fn load(provider: &mut impl ResourceProvider, name: &str) -> Result<Self> {
        let id = ResourceId::parse(name)?;
        let path = format!("data/{}/worldgen/carver/{}.json", id.namespace, id.path);
        let data = provider
            .read_data(&path)?
            .with_context(|| format!("missing carver {name}"))?;
        Self::decode(&data).with_context(|| format!("in carver {name}"))
    }
    pub fn decode(data: &[u8]) -> Result<Self> {
        let v: Value = serde_json::from_slice(data)?;
        ensure!(kind(&v)? == "cave", "not a cave carver");
        let probability = float(field(&v, "probability")?)?;
        ensure!((0.0..=1.0).contains(&probability), "invalid probability");
        let result = Self {
            probability,
            y: HeightProvider::parse(field(&v, "y")?)?,
            count: IntProvider::parse(field(&v, "count")?)?,
            thickness: FloatProvider::parse(field(&v, "thickness")?)?,
            weird: v
                .get("weird_thickness_bias")
                .map(|x| x.as_bool().context("invalid weird_thickness_bias"))
                .transpose()?
                .unwrap_or(false),
            room: FloatProvider::parse(field(&v, "room_vertical_radius_multiplier")?)?,
            horizontal: FloatProvider::parse(field(&v, "horizontal_radius_multiplier")?)?,
            vertical: FloatProvider::parse(field(&v, "vertical_radius_multiplier")?)?,
            start_vertical: v
                .get("start_vertical_radius_multiplier")
                .map(FloatProvider::parse)
                .transpose()?
                .unwrap_or(FloatProvider::Constant(1.0)),
            floor: FloatProvider::parse(field(&v, "floor_level")?)?,
        };
        ensure!(result.thickness.range().0 >= 0.0, "negative cave thickness");
        let (floor_min, floor_max) = result.floor.range();
        ensure!(floor_min >= -1.0 && floor_max <= 1.0, "invalid floor level");
        Ok(result)
    }
    /// Test and generate one source-chunk carver into the target chunk's mask.
    /// The caller must invoke carvers in biome-defined order for each source chunk.
    pub fn carve_source(
        &self,
        seed: i64,
        carver_index: i32,
        source: [i32; 2],
        target: [i32; 2],
        min_y: i32,
        height: i32,
        mask: &mut CarvingMask,
    ) -> Result<bool> {
        ensure!(height > 0, "invalid generation height");
        let Some(mut r) = source_random(seed, carver_index, source, self.probability) else {
            return Ok(false);
        };
        let count = self.count.sample(&mut r);
        for _ in 0..count {
            let x = f64::from(
                source[0]
                    .wrapping_mul(16)
                    .wrapping_add(r.next_int_bound(16).unwrap()),
            );
            let y = f64::from(self.y.sample(&mut r, min_y, height)?);
            let z = f64::from(
                source[1]
                    .wrapping_mul(16)
                    .wrapping_add(r.next_int_bound(16).unwrap()),
            );
            let hr = f64::from(self.horizontal.sample(&mut r));
            let vr = f64::from(self.vertical.sample(&mut r));
            let yscale = f64::from(self.start_vertical.sample(&mut r));
            let floor = f64::from(self.floor.sample(&mut r));
            let mut tunnels = 1;
            if r.next_int_bound(4).unwrap() == 0 {
                let scale = f64::from(self.room.sample(&mut r));
                let thickness = 1.0 + r.next_float() * 6.0;
                let radius =
                    1.5 + f64::from(sin(f64::from(std::f32::consts::PI / 2.0)) * thickness);
                ellipsoid(mask, target, [x + 1.0, y, z], radius, radius * scale, floor)?;
                tunnels += r.next_int_bound(4).unwrap();
            }
            for _ in 0..tunnels {
                let h = r.next_float() * (std::f32::consts::PI * 2.0);
                let v = (r.next_float() - 0.5) / 4.0;
                let mut t = self.thickness.sample(&mut r);
                if self.weird && r.next_int_bound(10).unwrap() == 0 {
                    t *= r.next_float() * r.next_float() * 3.0 + 1.0;
                }
                let distance = 112 - r.next_int_bound(28).unwrap();
                let seed = r.next_long();
                tunnel(
                    mask,
                    target,
                    seed,
                    [x, y, z],
                    hr,
                    vr,
                    t,
                    h,
                    v,
                    0,
                    distance,
                    yscale,
                    floor,
                )?;
            }
        }
        Ok(true)
    }
}
pub(super) fn sin(v: f64) -> f32 {
    trig(v, 0.0)
}
pub(super) fn cos(v: f64) -> f32 {
    trig(v, 16384.0)
}
fn trig(v: f64, offset: f64) -> f32 {
    static TABLE: OnceLock<Box<[f32]>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (0..65536)
            .map(|i| ((i as f64) / 10430.378350470453).sin() as f32)
            .collect()
    });
    table[((v * 10430.378350470453 + offset) as i64 & 65535) as usize]
}
fn ellipsoid(
    mask: &mut CarvingMask,
    chunk: [i32; 2],
    pos: [f64; 3],
    h: f64,
    v: f64,
    floor: f64,
) -> Result<()> {
    carve_ellipsoid(mask, chunk, pos, h, v, |x, y, z, _| {
        y <= floor || x * x + y * y + z * z >= 1.0
    })
}
#[allow(clippy::too_many_arguments)]
fn tunnel(
    mask: &mut CarvingMask,
    chunk: [i32; 2],
    seed: i64,
    mut pos: [f64; 3],
    hr: f64,
    vr: f64,
    thickness: f32,
    mut h: f32,
    mut v: f32,
    start: i32,
    distance: i32,
    scale: f64,
    floor: f64,
) -> Result<()> {
    let mut r = LegacyRandom::new(seed);
    let split = r.next_int_bound(distance / 2).unwrap() + distance / 4;
    let steep = r.next_int_bound(6).unwrap() == 0;
    let (mut yr, mut xr) = (0.0f32, 0.0f32);
    for step in start..distance {
        let radius = 1.5
            + f64::from(
                sin(f64::from(
                    std::f32::consts::PI * step as f32 / distance as f32,
                )) * thickness,
            );
        let cos_x = cos(f64::from(v));
        pos[0] += f64::from(cos(f64::from(h)) * cos_x);
        pos[1] += f64::from(sin(f64::from(v)));
        pos[2] += f64::from(sin(f64::from(h)) * cos_x);
        v *= if steep { 0.92 } else { 0.7 };
        v += xr * 0.1;
        h += yr * 0.1;
        xr *= 0.9;
        yr *= 0.75;
        xr += (r.next_float() - r.next_float()) * r.next_float() * 2.0;
        yr += (r.next_float() - r.next_float()) * r.next_float() * 4.0;
        if step == split && thickness > 1.0 {
            let seed = r.next_long();
            let t = r.next_float() * 0.5 + 0.5;
            tunnel(
                mask,
                chunk,
                seed,
                pos,
                hr,
                vr,
                t,
                h - std::f32::consts::PI / 2.0,
                v / 3.0,
                step,
                distance,
                1.0,
                floor,
            )?;
            let seed = r.next_long();
            let t = r.next_float() * 0.5 + 0.5;
            tunnel(
                mask,
                chunk,
                seed,
                pos,
                hr,
                vr,
                t,
                h + std::f32::consts::PI / 2.0,
                v / 3.0,
                step,
                distance,
                1.0,
                floor,
            )?;
            return Ok(());
        }
        if r.next_int_bound(4).unwrap() != 0 {
            if !can_reach(chunk, pos[0], pos[2], step, distance, thickness) {
                return Ok(());
            }
            ellipsoid(mask, chunk, pos, radius * hr, radius * scale * vr, floor)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_data_pack_and_rejects_unsupported_or_invalid_settings() {
        struct Data(Vec<u8>);
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok((path == "data/example/worldgen/carver/custom.json").then(|| self.0.clone()))
            }
        }
        let mut v = json!({"type":"minecraft:cave","probability":1.0,"y":{"absolute":32},"count":0,
            "thickness":1.0,"floor_level":-0.5,"room_vertical_radius_multiplier":1.0,
            "horizontal_radius_multiplier":1.0,"vertical_radius_multiplier":1.0});
        let mut data = Data(serde_json::to_vec(&v).unwrap());
        let c = CaveCarver::load(&mut data, "example:custom").unwrap();
        let mut mask = CarvingMask::new(-63, 312).unwrap();
        assert!(
            c.carve_source(42, 0, [0, 0], [0, 0], -64, 384, &mut mask)
                .unwrap()
        );
        assert!(mask.is_empty());
        assert!(CaveCarver::load(&mut data, "example:missing").is_err());
        v["count"] = json!({"type":"minecraft:unknown","min_inclusive":0,"max_inclusive":1});
        assert!(CaveCarver::decode(&serde_json::to_vec(&v).unwrap()).is_err());
        v["count"] = json!(0);
        v["floor_level"] = json!(2.0);
        assert!(CaveCarver::decode(&serde_json::to_vec(&v).unwrap()).is_err());
        v["floor_level"] = json!(0.0);
        v["thickness"] = json!(-1.0);
        assert!(CaveCarver::decode(&serde_json::to_vec(&v).unwrap()).is_err());
    }
}
