//! Decode 26.3 normal-noise definitions from the active data-pack stack.
use anyhow::{Context, Result, bail};
use native_core::normal_noise::{NormalNoiseParameters, Normalization};
use serde::Deserialize;

use crate::packs::{ResourceId, ResourceProvider};

#[derive(Deserialize)]
struct Definition {
    #[serde(default = "default_amplitude")]
    base_amplitude: f64,
    base_octave: i32,
    #[serde(default = "default_count")]
    octave_count: usize,
    #[serde(default)]
    amplitude_modifiers: Vec<f64>,
    #[serde(default)]
    normalize: Option<serde_json::Value>,
}
fn default_amplitude() -> f64 {
    1.0
}
fn default_count() -> usize {
    1
}

pub fn decode_normal_noise(bytes: &[u8]) -> Result<NormalNoiseParameters> {
    let definition: Definition =
        serde_json::from_slice(bytes).context("invalid normal noise JSON")?;
    let normalization = match definition.normalize.as_ref() {
        None | Some(serde_json::Value::Bool(true)) => Normalization::Enabled,
        Some(serde_json::Value::Bool(false)) => Normalization::Disabled,
        Some(serde_json::Value::String(value)) if value == "legacy" => Normalization::Legacy,
        _ => bail!("invalid normal noise normalization"),
    };
    let parameters = NormalNoiseParameters {
        base_amplitude: definition.base_amplitude,
        base_octave: definition.base_octave,
        octave_count: definition.octave_count,
        normalization,
        amplitude_modifiers: definition.amplitude_modifiers,
    };
    parameters.validate().map_err(anyhow::Error::msg)?;
    Ok(parameters)
}

pub fn load_normal_noise(
    provider: &mut impl ResourceProvider,
    name: &str,
) -> Result<NormalNoiseParameters> {
    let id = ResourceId::parse(name)?;
    let path = format!("data/{}/worldgen/noise/{}.json", id.namespace, id.path);
    let bytes = provider
        .read_data(&path)?
        .with_context(|| format!("missing normal noise {name}"))?;
    decode_normal_noise(&bytes).with_context(|| format!("in normal noise {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_noise_parameters_and_defaults() {
        let defaults = decode_normal_noise(br#"{"base_octave":-7}"#).unwrap();
        assert_eq!(defaults.base_amplitude, 1.0);
        assert_eq!(defaults.octave_count, 1);
        assert_eq!(defaults.normalization, Normalization::Enabled);
        assert!(
            decode_normal_noise(
                br#"{"base_octave":-7,"octave_count":2,"amplitude_modifiers":[1]}"#
            )
            .is_err()
        );
        assert!(decode_normal_noise(br#"{"base_octave":-33}"#).is_err());
        assert!(decode_normal_noise(br#"{"base_octave":0,"normalize":"unknown"}"#).is_err());
    }

    #[test]
    fn resolves_the_selected_data_provider_and_reports_missing_noise() {
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok((path == "data/custom/worldgen/noise/test.json").then(|| {
                    br#"{"base_octave":-4,"octave_count":2,"normalize":"legacy"}"#.to_vec()
                }))
            }
        }
        let mut data = Data;
        let noise = load_normal_noise(&mut data, "custom:test").unwrap();
        assert_eq!(noise.base_octave, -4);
        assert_eq!(noise.normalization, Normalization::Legacy);
        assert!(load_normal_noise(&mut data, "custom:missing").is_err());
        assert!(load_normal_noise(&mut data, "custom:../test").is_err());
    }
}
