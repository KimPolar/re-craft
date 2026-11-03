//! The noise generator's data-pack-driven initial spawn origin in Java 26.3.
use crate::{
    density::{DensityCompiler, DensityEvaluator, DensityFunction, WorldgenRandom},
    packs::{ResourceId, ResourceProvider},
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

struct Parameter {
    function: usize,
    min: i64,
    max: i64,
}

/// Compiled targets from the generator's settings and active data packs.
pub struct NoiseSpawnTargets {
    functions: Vec<Arc<DensityFunction>>,
    targets: Vec<Vec<Parameter>>,
}

impl NoiseSpawnTargets {
    pub fn compile(
        provider: &mut impl ResourceProvider,
        target: &Value,
        seed: i64,
        random: WorldgenRandom,
    ) -> Result<Self> {
        let entries = target.as_array().context("spawn_target must be a list")?;
        let mut compiler = DensityCompiler::with_world_seed(provider, seed, random)
            .without_old_chunk_blending()
            .without_structure_terrain_adjustment();
        let mut targets = Vec::with_capacity(entries.len());
        let mut functions = Vec::new();
        let mut function_indices = BTreeMap::new();
        for entry in entries {
            let mut parameters = Vec::new();
            for (name, range) in entry.as_object().context("spawn target must be a map")? {
                let id = ResourceId::parse(name)?;
                let canonical = format!("{}:{}", id.namespace, id.path);
                let (min, max) = parameter_range(range)?;
                let function = if let Some(&index) = function_indices.get(&id) {
                    index
                } else {
                    let index = functions.len();
                    functions.push(compiler.compile(&Value::String(canonical))?);
                    function_indices.insert(id, index);
                    index
                };
                parameters.push(Parameter { function, min, max });
            }
            targets.push(parameters);
        }
        Ok(Self { functions, targets })
    }

    /// Return chunk coordinates, using Java's quart sampling and f32 quantization.
    pub fn origin_chunk(&self, evaluator: &mut DensityEvaluator) -> [i32; 2] {
        if self.targets.is_empty() {
            return [0, 0];
        }
        let mut sampled = Vec::with_capacity(self.functions.len());
        let mut evaluate = |x: i32, z: i32| {
            let qx = (x >> 2) << 2;
            let qz = (z >> 2) << 2;
            evaluator.sample_slice(&self.functions, qx, 0, qz, &mut sampled);
            let best = self
                .targets
                .iter()
                .map(|target| {
                    target.iter().fold(0_i64, |sum, p| {
                        let value = quantize(sampled[p.function]);
                        let above = value.wrapping_sub(p.max);
                        let below = p.min.wrapping_sub(value);
                        let distance = if above > 0 { above } else { below.max(0) };
                        sum.wrapping_add(distance.wrapping_mul(distance))
                    })
                })
                .min()
                .unwrap();
            best.wrapping_mul(2048_i64 * 2048)
                .wrapping_add(i64::from(x) * i64::from(x))
                .wrapping_add(i64::from(z) * i64::from(z))
        };
        let mut result = (0_i32, 0_i32, evaluate(0, 0));
        radial_search(&mut result, 2048.0, 512.0, &mut evaluate);
        radial_search(&mut result, 512.0, 32.0, &mut evaluate);
        [result.0.div_euclid(16), result.1.div_euclid(16)]
    }
}

fn radial_search(
    result: &mut (i32, i32, i64),
    max_radius: f32,
    step: f32,
    evaluate: &mut impl FnMut(i32, i32) -> i64,
) {
    let (center_x, center_z) = (result.0, result.1);
    let (mut angle, mut radius) = (0.0_f32, step);
    while radius <= max_radius {
        let x = center_x + ((f64::from(angle).sin() * f64::from(radius)) as i32);
        let z = center_z + ((f64::from(angle).cos() * f64::from(radius)) as i32);
        let fitness = evaluate(x, z);
        if fitness < result.2 {
            *result = (x, z, fitness);
        }
        angle += step / radius;
        if f64::from(angle) > std::f64::consts::TAU {
            angle = 0.0;
            radius += step;
        }
    }
}
fn quantize(value: f32) -> i64 {
    (value * 10_000.0_f32) as i64
}
fn parameter_range(value: &Value) -> Result<(i64, i64)> {
    fn number(value: &Value) -> Result<f32> {
        let n = value.as_f64().context("spawn parameter must be a number")? as f32;
        ensure!(
            n.is_finite() && (-2.0..=2.0).contains(&n),
            "spawn parameter out of range"
        );
        Ok(n)
    }
    let (min, max) = match value {
        Value::Number(_) => {
            let n = number(value)?;
            (n, n)
        }
        Value::Array(pair) if pair.len() == 2 => (number(&pair[0])?, number(&pair[1])?),
        Value::Object(map) => (
            number(map.get("min").context("missing min")?)?,
            number(map.get("max").context("missing max")?)?,
        ),
        _ => anyhow::bail!("invalid spawn parameter"),
    };
    ensure!(min <= max, "reversed spawn parameter");
    Ok((quantize(min), quantize(max)))
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
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(
                (path == "data/minecraft/worldgen/density_function/custom.json")
                    .then(|| b"0.25".to_vec()),
            )
        }
    }

    #[test]
    fn targets_use_pack_functions_and_reject_invalid_ranges() {
        let compile = |target: Value| {
            NoiseSpawnTargets::compile(&mut Data, &target, 17, WorldgenRandom::Xoroshiro)
        };
        assert_eq!(
            compile(json!([]))
                .unwrap()
                .origin_chunk(&mut DensityEvaluator::default()),
            [0, 0]
        );
        assert_eq!(
            compile(json!([{"minecraft:custom": [0.0, 0.5]}]))
                .unwrap()
                .origin_chunk(&mut DensityEvaluator::default()),
            [0, 0]
        );
        assert!(compile(json!([{"minecraft:custom": [0.5, -0.5]}])).is_err());
        assert!(compile(json!([{"minecraft:custom": [0, 2.1]}])).is_err());
        assert!(compile(json!([{"minecraft:unknown": 0.0}])).is_err());
        assert!(compile(json!({})).is_err());
        assert_eq!(
            parameter_range(&json!([-0.12349, 0.12349])).unwrap(),
            (-1234, 1234)
        );
    }
}
