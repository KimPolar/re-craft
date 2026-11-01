//! Data-driven 26.3 canyon carving masks.
use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::cave::{FloatProvider, HeightProvider, cos, field, float, int, kind, sin};
use super::{CarvingMask, can_reach, carve_ellipsoid, source_random};
use crate::packs::{ResourceId, ResourceProvider};

#[derive(Clone, Debug)]
pub struct CanyonCarver {
    probability: f32,
    y: HeightProvider,
    vertical_rotation: FloatProvider,
    distance_factor: FloatProvider,
    thickness: FloatProvider,
    width_smoothness: i32,
    horizontal_radius_factor: FloatProvider,
    vertical_radius_default_factor: f32,
    vertical_radius_center_factor: f32,
    y_scale: FloatProvider,
}

impl CanyonCarver {
    pub fn load(provider: &mut impl ResourceProvider, name: &str) -> Result<Self> {
        let id = ResourceId::parse(name)?;
        let path = format!("data/{}/worldgen/carver/{}.json", id.namespace, id.path);
        let data = provider
            .read_data(&path)?
            .with_context(|| format!("missing carver {name}"))?;
        Self::decode(&data).with_context(|| format!("in carver {name}"))
    }
    pub fn decode(data: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(data)?;
        ensure!(kind(&value)? == "canyon", "not a canyon carver");
        let probability = float(field(&value, "probability")?)?;
        ensure!((0.0..=1.0).contains(&probability), "invalid probability");
        let shape = field(&value, "shape")?;
        let width_smoothness = int(field(shape, "width_smoothness")?)?;
        ensure!(width_smoothness > 0, "invalid width smoothness");
        Ok(Self {
            probability,
            y: HeightProvider::parse(field(&value, "y")?)?,
            vertical_rotation: FloatProvider::parse(field(&value, "vertical_rotation")?)?,
            distance_factor: FloatProvider::parse(field(shape, "distance_factor")?)?,
            thickness: FloatProvider::parse(field(shape, "thickness")?)?,
            width_smoothness,
            horizontal_radius_factor: FloatProvider::parse(field(
                shape,
                "horizontal_radius_factor",
            )?)?,
            vertical_radius_default_factor: float(field(shape, "vertical_radius_default_factor")?)?,
            vertical_radius_center_factor: float(field(shape, "vertical_radius_center_factor")?)?,
            y_scale: FloatProvider::parse(field(shape, "y_scale")?)?,
        })
    }
    /// Generate one source chunk's contribution into a target-chunk mask.
    #[allow(clippy::too_many_arguments)]
    pub fn carve_source(
        &self,
        seed: i64,
        index: i32,
        source: [i32; 2],
        target: [i32; 2],
        min_y: i32,
        height: i32,
        mask: &mut CarvingMask,
    ) -> Result<bool> {
        let end = min_y
            .checked_add(height)
            .context("generation height overflow")?;
        ensure!(
            height > 0 && height <= 4096 && mask.min_y() > min_y && mask.max_y() < end,
            "mask outside generation height"
        );
        let Some(mut r) = source_random(seed, index, source, self.probability) else {
            return Ok(false);
        };
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
        let h = r.next_float() * (std::f32::consts::PI * 2.0);
        let v = self.vertical_rotation.sample(&mut r);
        let scale = f64::from(self.y_scale.sample(&mut r));
        let thickness = self.thickness.sample(&mut r);
        let distance = (112.0f32 * self.distance_factor.sample(&mut r)) as i32;
        let tunnel_seed = r.next_long();
        self.carve_tunnel(
            mask,
            target,
            tunnel_seed,
            [x, y, z],
            h,
            v,
            scale,
            thickness,
            distance,
            min_y,
            height,
        )?;
        Ok(true)
    }
    #[allow(clippy::too_many_arguments)]
    fn carve_tunnel(
        &self,
        mask: &mut CarvingMask,
        chunk: [i32; 2],
        seed: i64,
        mut pos: [f64; 3],
        mut horizontal: f32,
        mut vertical: f32,
        scale: f64,
        thickness: f32,
        distance: i32,
        min_y: i32,
        height: i32,
    ) -> Result<()> {
        use native_core::random::LegacyRandom;
        let mut r = LegacyRandom::new(seed);
        let mut widths = Vec::with_capacity(height as usize);
        let mut width = 1.0f32;
        for y in 0..height {
            if y == 0 || r.next_int_bound(self.width_smoothness).unwrap() == 0 {
                width = 1.0 + r.next_float() * r.next_float();
            }
            widths.push(width * width);
        }
        let (mut yr, mut xr) = (0.0f32, 0.0f32);
        for step in 0..distance {
            let radius = 1.5
                + f64::from(
                    sin(f64::from(
                        step as f32 * std::f32::consts::PI / distance as f32,
                    )) * thickness,
                );
            let horizontal_radius =
                radius * f64::from(self.horizontal_radius_factor.sample(&mut r));
            let center = 1.0 - (0.5 - step as f32 / distance as f32).abs() * 2.0;
            let factor =
                self.vertical_radius_default_factor + self.vertical_radius_center_factor * center;
            let vertical_radius =
                f64::from(factor) * (radius * scale) * f64::from(0.75 + r.next_float() * 0.25);
            let xc = cos(f64::from(vertical));
            pos[0] += f64::from(cos(f64::from(horizontal)) * xc);
            pos[1] += f64::from(sin(f64::from(vertical)));
            pos[2] += f64::from(sin(f64::from(horizontal)) * xc);
            vertical *= 0.7;
            vertical += xr * 0.05;
            horizontal += yr * 0.05;
            xr *= 0.8;
            yr *= 0.5;
            xr += (r.next_float() - r.next_float()) * r.next_float() * 2.0;
            yr += (r.next_float() - r.next_float()) * r.next_float() * 4.0;
            if r.next_int_bound(4).unwrap() != 0 {
                if !can_reach(chunk, pos[0], pos[2], step, distance, thickness) {
                    return Ok(());
                }
                carve_ellipsoid(
                    mask,
                    chunk,
                    pos,
                    horizontal_radius,
                    vertical_radius,
                    |x, y, z, by| {
                        (x * x + z * z) * f64::from(widths[(by - min_y - 1) as usize]) + y * y / 6.0
                            >= 1.0
                    },
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn rejects_invalid_settings_and_mask_height() {
        let mut v = json!({"type":"canyon","probability":1,"y":{"absolute":20},"vertical_rotation":0,
          "shape":{"distance_factor":1,"thickness":2,"width_smoothness":3,"horizontal_radius_factor":1,
          "vertical_radius_default_factor":1,"vertical_radius_center_factor":0,"y_scale":2}});
        let c = CanyonCarver::decode(&serde_json::to_vec(&v).unwrap()).unwrap();
        let mut mask = CarvingMask::new(-64, 80).unwrap();
        assert!(
            c.carve_source(0, 0, [0, 0], [0, 0], -64, 384, &mut mask)
                .is_err()
        );
        v["shape"]["width_smoothness"] = json!(0);
        assert!(CanyonCarver::decode(&serde_json::to_vec(&v).unwrap()).is_err());
    }
}
