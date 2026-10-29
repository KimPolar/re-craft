//! Static dimension lighting for the read-only world viewer.

use anyhow::{Context, Result, bail, ensure};
use native_persistence::world::{DimensionId, SavedDimensionType};
use native_resources::packs::{ResourceId, ResourceProvider};
use native_world::lighting::LightmapSettings;
use serde_json::Value;
use std::collections::BTreeMap;

/// Java bakes the active data-pack dimension registry before falling back to
/// the level stems stored in world_gen_settings.dat.
pub fn resolve_dimension_type(
    source: &mut impl ResourceProvider,
    dimension: &DimensionId,
    saved: Option<&BTreeMap<DimensionId, SavedDimensionType>>,
) -> Result<Option<String>> {
    let path = format!(
        "data/{}/dimension/{}.json",
        dimension.namespace, dimension.path
    );
    if let Some(bytes) = source.read_data(&path)? {
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid dimension definition {path}"))?;
        ensure!(
            value.get("generator").is_some_and(Value::is_object),
            "dimension {path} has no generator"
        );
        let name = value.get("type").and_then(Value::as_str).with_context(|| {
            format!("inline or missing dimension type in {path} is not supported")
        })?;
        let id = ResourceId::parse(name)?;
        return Ok(Some(format!("{}:{}", id.namespace, id.path)));
    }
    match saved.and_then(|map| map.get(dimension)) {
        Some(SavedDimensionType::Named(id)) => Ok(Some(format!("{}:{}", id.namespace, id.path))),
        Some(SavedDimensionType::Inline(_)) => {
            bail!("inline saved dimension types are not supported")
        }
        None => Ok(None),
    }
}

#[derive(Clone, Debug)]
pub struct PreviewLighting {
    pub lightmap: LightmapSettings,
    pub has_skylight: bool,
    pub has_ceiling: Option<bool>,
    pub nether_cardinal: bool,
    pub build_bounds: Option<(i32, i32)>,
}

impl Default for PreviewLighting {
    fn default() -> Self {
        Self {
            lightmap: LightmapSettings::default(),
            has_skylight: true,
            has_ceiling: None,
            nether_cardinal: false,
            build_bounds: None,
        }
    }
}

pub(crate) fn rgb(value: &Value) -> Result<[f32; 3]> {
    let packed = if let Some(s) = value.as_str() {
        ensure!(s.len() == 7 && s.starts_with('#'), "color must be #RRGGBB");
        u32::from_str_radix(&s[1..], 16).context("invalid RGB color")?
    } else {
        let n = value
            .as_u64()
            .context("RGB color must be a string or integer")?;
        ensure!(n <= 0xff_ffff, "RGB color is out of range");
        n as u32
    };
    Ok([
        ((packed >> 16) & 255) as f32 / 255.0,
        ((packed >> 8) & 255) as f32 / 255.0,
        (packed & 255) as f32 / 255.0,
    ])
}

/// Time-dependent sky-light layers whose spatial inputs are constant.
pub struct SkyTimeline {
    tracks: Vec<SkyTrack>,
}

pub(crate) struct SkyTrack {
    pub(crate) clock: ResourceId,
    period: Option<i32>,
    factor: bool,
    pub(crate) multiply: bool,
    constant_easing: bool,
    frames: Vec<(i32, [f32; 3])>,
}

impl SkyTimeline {
    pub fn is_animated(&self) -> bool {
        !self.tracks.is_empty()
    }
    pub fn load(source: &mut impl ResourceProvider, dimension_type: &str) -> Result<Self> {
        for id in source.list_data("worldgen/biome")? {
            let path = format!("data/{}/worldgen/biome/{}.json", id.namespace, id.path);
            let bytes = source
                .read_data(&path)?
                .with_context(|| format!("missing {path}"))?;
            let biome: Value =
                serde_json::from_slice(&bytes).with_context(|| format!("invalid {path}"))?;
            if let Some(attributes) = biome.get("attributes") {
                let attributes = attributes.as_object().context("invalid biome attributes")?;
                for name in attributes.keys() {
                    let id = ResourceId::parse(name)?;
                    ensure!(
                        id.namespace != "minecraft"
                            || !matches!(
                                id.path.as_str(),
                                "visual/sky_light_color"
                                    | "visual/sky_light_factor"
                                    | "visual/block_light_tint"
                                    | "visual/ambient_light_color"
                                    | "visual/night_vision_color"
                            ),
                        "biome-dependent lightmap attributes are not yet supported"
                    );
                }
            }
        }
        let timelines =
            native_resources::fluid_environment::load_dimension_timelines(source, dimension_type)?;
        let mut result = Self { tracks: Vec::new() };
        for timeline in timelines {
            let tracks = timeline
                .get("tracks")
                .and_then(Value::as_object)
                .context("invalid timeline tracks")?;
            let mut seen = std::collections::BTreeSet::new();
            for (name, value) in tracks {
                let id = ResourceId::parse(name)?;
                ensure!(
                    seen.insert(id.clone()),
                    "duplicate timeline attribute {name}"
                );
                if id.namespace != "minecraft" {
                    continue;
                }
                match id.path.as_str() {
                    "visual/sky_light_factor" => {
                        result.tracks.push(SkyTrack::parse(&timeline, value, true)?)
                    }
                    "visual/sky_light_color" => result
                        .tracks
                        .push(SkyTrack::parse(&timeline, value, false)?),
                    "visual/block_light_tint"
                    | "visual/ambient_light_color"
                    | "visual/night_vision_color" => bail!("unsupported lightmap timeline {name}"),
                    _ => {}
                }
            }
        }
        Ok(result)
    }

    pub fn sample(
        &self,
        base: &LightmapSettings,
        mut clock_ticks: impl FnMut(&ResourceId) -> Option<i64>,
    ) -> Option<LightmapSettings> {
        let mut result = base.clone();
        for track in &self.tracks {
            let value = track.sample(clock_ticks(&track.clock)?);
            if track.factor {
                result.sky_factor = if track.multiply {
                    result.sky_factor * value[0]
                } else {
                    value[0]
                };
            } else {
                for (channel, value) in result.sky_color.iter_mut().zip(value) {
                    *channel = if track.multiply {
                        *channel * value
                    } else {
                        value
                    };
                }
            }
        }
        result.sky_factor = result.sky_factor.clamp(0.0, 1.0);
        Some(result)
    }
}

impl SkyTrack {
    pub(crate) fn parse(timeline: &Value, track: &Value, factor: bool) -> Result<Self> {
        let clock = ResourceId::parse(
            timeline
                .get("clock")
                .and_then(Value::as_str)
                .context("missing clock")?,
        )?;
        let period = timeline
            .get("period_ticks")
            .map(|v| {
                let n = v
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .context("invalid period")?;
                ensure!(n > 0, "invalid period");
                Ok(n)
            })
            .transpose()?;
        let modifier = match track.get("modifier") {
            None => "override",
            Some(value) => value.as_str().context("invalid sky light modifier")?,
        };
        let multiply = match modifier {
            "override" | "minecraft:override" => false,
            "multiply" | "minecraft:multiply" => true,
            _ => bail!("unsupported sky light modifier"),
        };
        let constant_easing = match track.get("ease") {
            None => false,
            Some(Value::String(s)) if s == "linear" => false,
            Some(Value::String(s)) if s == "constant" => true,
            _ => bail!("unsupported sky light easing"),
        };
        let values = track
            .get("keyframes")
            .and_then(Value::as_array)
            .context("missing keyframes")?;
        ensure!(!values.is_empty(), "empty keyframes");
        let mut frames: Vec<(i32, [f32; 3])> = Vec::with_capacity(values.len());
        for frame in values {
            let tick = frame
                .get("ticks")
                .and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok())
                .context("invalid keyframe tick")?;
            if let Some(period) = period {
                ensure!((0..=period).contains(&tick), "keyframe outside period");
            }
            if let Some((previous, _)) = frames.last() {
                ensure!(tick >= *previous, "unordered keyframes");
            }
            if frames.len() >= 2 {
                ensure!(
                    frames[frames.len() - 2].0 != tick,
                    "three equal keyframe ticks"
                );
            }
            let value = frame.get("value").context("missing keyframe value")?;
            let value = if factor {
                [value.as_f64().context("invalid sky factor")? as f32; 3]
            } else {
                rgb(value)?
            };
            ensure!(
                value.iter().all(|v| v.is_finite()),
                "non-finite sky keyframe"
            );
            frames.push((tick, value));
        }
        Ok(Self {
            clock,
            period,
            factor,
            multiply,
            constant_easing,
            frames,
        })
    }

    pub(crate) fn sample(&self, ticks: i64) -> [f32; 3] {
        let ticks = self
            .period
            .map_or(ticks, |p| ticks.rem_euclid(i64::from(p)));
        let first = self.frames[0];
        let last = self.frames[self.frames.len() - 1];
        if self.frames.len() == 1 {
            return first.1;
        }
        let before = self
            .period
            .map(|p| ((last.0.wrapping_sub(p), last.1), first));
        let after = self
            .period
            .map(|p| (last, (first.0.wrapping_add(p), first.1)));
        let segments = before
            .into_iter()
            .chain(self.frames.windows(2).map(|w| (w[0], w[1])))
            .chain(after);
        let ((start, a), (end, b)) = segments
            .clone()
            .find(|(_, end)| ticks < i64::from(end.0))
            .unwrap_or_else(|| segments.last().unwrap());
        if ticks <= i64::from(start) {
            return a;
        }
        if ticks >= i64::from(end) {
            return b;
        }
        if self.constant_easing {
            return a;
        }
        let delta = ticks.wrapping_sub(i64::from(start)) as f32 / end.wrapping_sub(start) as f32;
        std::array::from_fn(|i| a[i] + delta * (b[i] - a[i]))
    }
}

impl PreviewLighting {
    pub fn fallback(dimension: &str) -> Self {
        let mut result = Self::default();
        if dimension == "minecraft:the_nether" {
            result.has_skylight = false;
            result.nether_cardinal = true;
            result.lightmap.sky_factor = 0.0;
        } else if dimension == "minecraft:the_end" {
            result.lightmap.sky_factor = 0.0;
        }
        result
    }

    /// Fallback for worlds without a resolved dimension-type mapping.
    pub fn load(source: &mut impl ResourceProvider, dimension: &str) -> Result<Self> {
        let name = match dimension {
            "minecraft:overworld" => "overworld",
            "minecraft:the_nether" => "the_nether",
            "minecraft:the_end" => "the_end",
            _ => bail!("dimension-to-type mapping is not implemented for {dimension}"),
        };
        Self::load_type(source, &format!("minecraft:{name}"))
    }

    /// Read the resolved dimension type using active data-pack precedence.
    pub fn load_type(source: &mut impl ResourceProvider, dimension_type: &str) -> Result<Self> {
        let id = ResourceId::parse(dimension_type)?;
        let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
        let bytes = source
            .read_data(&path)?
            .with_context(|| format!("missing {path}"))?;
        Self::from_json(&bytes)
    }

    fn from_json(bytes: &[u8]) -> Result<Self> {
        let json: Value = serde_json::from_slice(bytes).context("invalid dimension type JSON")?;
        let has_skylight = json
            .get("has_skylight")
            .and_then(Value::as_bool)
            .context("dimension type is missing has_skylight")?;
        let has_ceiling = json
            .get("has_ceiling")
            .map(|value| {
                value
                    .as_bool()
                    .context("dimension has_ceiling must be a boolean")
            })
            .transpose()?;
        let build_bounds = match (json.get("min_y"), json.get("height")) {
            (None, None) => None,
            (Some(min), Some(height)) => {
                let min = min.as_i64().context("dimension min_y must be an integer")?;
                let height = height
                    .as_i64()
                    .context("dimension height must be an integer")?;
                let max = min
                    .checked_add(height)
                    .context("dimension height overflows")?;
                ensure!(
                    height > 0 && min % 16 == 0 && height % 16 == 0 && min >= -2048 && max <= 2048,
                    "dimension bounds are outside the supported section range"
                );
                Some((min as i32, max as i32))
            }
            _ => bail!("dimension min_y and height must both be present"),
        };
        let nether_cardinal = match json.get("cardinal_light") {
            None => false,
            Some(Value::String(value)) if value == "default" => false,
            Some(Value::String(value)) if value == "nether" => true,
            Some(value) => bail!("unsupported cardinal lighting {value}"),
        };
        let mut lightmap = LightmapSettings {
            ambient_color: [0.0; 3],
            ..LightmapSettings::default()
        };
        if let Some(attributes) = json.get("attributes") {
            let attributes = attributes
                .as_object()
                .context("dimension attributes must be an object")?;
            if let Some(value) = attributes.get("minecraft:visual/block_light_tint") {
                lightmap.block_tint = rgb(value)?;
            }
            if let Some(value) = attributes.get("minecraft:visual/sky_light_color") {
                lightmap.sky_color = rgb(value)?;
            }
            if let Some(value) = attributes.get("minecraft:visual/ambient_light_color") {
                lightmap.ambient_color = rgb(value)?;
            }
            if let Some(value) = attributes.get("minecraft:visual/night_vision_color") {
                lightmap.night_vision_color = rgb(value)?;
            }
            if let Some(value) = attributes.get("minecraft:visual/sky_light_factor") {
                let factor = value
                    .as_f64()
                    .context("sky light factor must be a number")?;
                ensure!(
                    (0.0..=1.0).contains(&factor),
                    "sky light factor is out of range"
                );
                lightmap.sky_factor = factor as f32;
            }
        }
        Ok(Self {
            lightmap,
            has_skylight,
            has_ceiling,
            nether_cardinal,
            build_bounds,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_timeline_uses_active_tracks_and_rejects_biome_overrides() {
        use serde_json::json;
        struct Data {
            biome: Value,
        }
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/example/worldgen/biome/test.json" => self.biome.clone(),
                    "data/example/dimension_type/test.json" => json!({"timelines":["example:day"]}),
                    "data/example/timeline/day.json" => {
                        json!({"clock":"example:clock","period_ticks":20,"tracks":{
                            "visual/sky_light_factor":{"modifier":"multiply","keyframes":[{"ticks":0,"value":1.0},{"ticks":10,"value":0.25}]},
                            "visual/sky_light_color":{"keyframes":[{"ticks":0,"value":"#ffffff"},{"ticks":10,"value":"#000000"}]}
                        }})
                    }
                    _ => return Ok(None),
                };
                Ok(Some(serde_json::to_vec(&value)?))
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                if folder == "worldgen/biome" {
                    Ok(vec![ResourceId::parse("example:test")?])
                } else {
                    Ok(vec![])
                }
            }
        }
        let mut data = Data { biome: json!({}) };
        let environment = SkyTimeline::load(&mut data, "example:test").unwrap();
        let base = LightmapSettings {
            sky_factor: 0.8,
            sky_color: [0.5; 3],
            ..Default::default()
        };
        let settings = environment.sample(&base, |_| Some(5)).unwrap();
        assert!((settings.sky_factor - 0.5).abs() < 0.000001);
        assert_eq!(settings.sky_color, [0.5; 3]);
        assert!(environment.sample(&base, |_| None).is_none());
        data.biome = json!({"attributes":{"minecraft:visual/sky_light_factor":0.5}});
        assert!(SkyTimeline::load(&mut data, "example:test").is_err());
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn sky_timeline_matches_installed_java_sampler() {
        use native_resources::packs::ResourceStack;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let mut resources = ResourceStack::open(&game).unwrap();
        let day: Value = serde_json::from_slice(
            &resources
                .read_data("data/minecraft/timeline/day.json")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let factor = &day["tracks"]["minecraft:visual/sky_light_factor"];
        let color = &day["tracks"]["minecraft:visual/sky_light_color"];
        let ticks: Vec<i64> = (-1000..=25000)
            .step_by(37)
            .chain([730, 11270, 13140, 22860, 0, 24000])
            .collect();
        let input = serde_json::json!({"factor":factor,"color":color,"ticks":ticks});
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("SkyTimelineOracle.java");
        let input_file = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/SkyTimelineOracle.java"),
        )
        .unwrap();
        std::fs::write(&input_file, serde_json::to_vec(&input).unwrap()).unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java sky oracle: {} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java"))
            .arg("-cp")
            .arg(&cp)
            .arg("SkyTimelineOracle")
            .arg(&input_file)
            .arg(&output));
        let oracle: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let environment = SkyTimeline {
            tracks: vec![
                SkyTrack::parse(&day, factor, true).unwrap(),
                SkyTrack::parse(&day, color, false).unwrap(),
            ],
        };
        let base = LightmapSettings {
            sky_factor: 1.0,
            sky_color: [1.0; 3],
            ..Default::default()
        };
        assert_eq!(ticks.len(), oracle.len());
        for (tick, row) in ticks.into_iter().zip(oracle) {
            let actual = environment.sample(&base, |_| Some(tick)).unwrap();
            assert!(
                (actual.sky_factor - row["factor"].as_f64().unwrap() as f32).abs() < 0.000001,
                "tick {tick}"
            );
            for i in 0..3 {
                assert!(
                    (actual.sky_color[i] - row["color"][i].as_f64().unwrap() as f32).abs()
                        < 0.000001,
                    "tick {tick}, channel {i}"
                );
            }
        }
        assert!(
            SkyTimeline::load(&mut resources, "minecraft:overworld")
                .unwrap()
                .is_animated()
        );
        assert!(
            !SkyTimeline::load(&mut resources, "minecraft:the_nether")
                .unwrap()
                .is_animated()
        );
    }

    #[test]
    fn active_dimension_registry_precedes_saved_stems() {
        struct Provider(Option<Vec<u8>>);
        impl ResourceProvider for Provider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                assert_eq!(path, "data/example/dimension/deep/cavern.json");
                Ok(self.0.clone())
            }
        }
        let dimension = DimensionId::parse("example:deep/cavern").unwrap();
        let saved = BTreeMap::from([(
            dimension.clone(),
            SavedDimensionType::Named(DimensionId::parse("minecraft:the_nether").unwrap()),
        )]);
        assert_eq!(
            resolve_dimension_type(&mut Provider(None), &dimension, Some(&saved))
                .unwrap()
                .as_deref(),
            Some("minecraft:the_nether")
        );
        let override_json =
            br#"{"type":"example:custom_type","generator":{"type":"minecraft:noise"}}"#;
        assert_eq!(
            resolve_dimension_type(
                &mut Provider(Some(override_json.to_vec())),
                &dimension,
                Some(&saved)
            )
            .unwrap()
            .as_deref(),
            Some("example:custom_type")
        );
        assert!(
            resolve_dimension_type(
                &mut Provider(Some(br#"{"type":{},"generator":{}}"#.to_vec())),
                &dimension,
                Some(&saved)
            )
            .is_err()
        );
        assert!(
            resolve_dimension_type(
                &mut Provider(Some(br#"{"type":"minecraft:overworld"}"#.to_vec())),
                &dimension,
                Some(&saved)
            )
            .is_err()
        );
        assert_eq!(
            resolve_dimension_type(&mut Provider(None), &dimension, None).unwrap(),
            None
        );
    }

    #[test]
    fn reads_dimension_attributes_and_cardinal_light() {
        let lighting = PreviewLighting::from_json(
            br##"{
            "has_skylight":false,"cardinal_light":"nether",
            "attributes":{"minecraft:visual/sky_light_factor":0.0,
                          "minecraft:visual/ambient_light_color":"#302821",
                          "minecraft:visual/block_light_tint":16767116}
        }"##,
        )
        .unwrap();
        assert!(!lighting.has_skylight);
        assert!(lighting.nether_cardinal);
        assert_eq!(lighting.lightmap.sky_factor, 0.0);
        assert_eq!(
            lighting.lightmap.ambient_color,
            [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0]
        );
        assert_eq!(
            lighting.lightmap.block_tint,
            [1.0, 216.0 / 255.0, 140.0 / 255.0]
        );
        assert!(
            PreviewLighting::from_json(br#"{"has_skylight":true,"cardinal_light":"unknown"}"#)
                .is_err()
        );
        assert!(
            PreviewLighting::from_json(br#"{"has_skylight":true,"cardinal_light":42}"#).is_err()
        );
    }

    #[test]
    #[ignore = "requires the user's Minecraft Java Edition 26.3 installation"]
    fn builtin_dimensions_and_data_pack_overrides_are_used() {
        use native_resources::{datapacks::DataPackSettings, packs::ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = ResourceStack::open(&game).unwrap();
        let overworld = PreviewLighting::load(&mut source, "minecraft:overworld").unwrap();
        assert!(overworld.has_skylight && !overworld.nether_cardinal);
        assert_eq!(overworld.lightmap.ambient_color, [10.0 / 255.0; 3]);
        let nether = PreviewLighting::load(&mut source, "minecraft:the_nether").unwrap();
        assert!(!nether.has_skylight && nether.nether_cardinal);
        assert_eq!(nether.lightmap.sky_factor, 0.0);
        assert_eq!(
            nether.lightmap.ambient_color,
            [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0]
        );
        let end = PreviewLighting::load(&mut source, "minecraft:the_end").unwrap();
        assert!(end.has_skylight && !end.nether_cardinal);
        assert_eq!(end.lightmap.sky_factor, 0.0);
        assert_eq!(
            end.lightmap.ambient_color,
            [63.0 / 255.0, 71.0 / 255.0, 63.0 / 255.0]
        );

        let temp = tempfile::tempdir().unwrap();
        let pack = temp.path().join("datapacks/dimension_override");
        let dimension_dir = pack.join("data/minecraft/dimension_type");
        std::fs::create_dir_all(&dimension_dir).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        let mut dimension: Value = serde_json::from_slice(
            &source
                .read_builtin_data("data/minecraft/dimension_type/overworld.json")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        dimension["attributes"]["minecraft:visual/ambient_light_color"] = "#123456".into();
        std::fs::write(
            dimension_dir.join("overworld.json"),
            serde_json::to_vec(&dimension).unwrap(),
        )
        .unwrap();
        let warnings = source
            .configure_world_data(
                temp.path(),
                &DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/dimension_override".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )
            .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let overridden = PreviewLighting::load(&mut source, "minecraft:overworld").unwrap();
        assert_eq!(
            overridden.lightmap.ambient_color,
            [18.0 / 255.0, 52.0 / 255.0, 86.0 / 255.0]
        );
    }
}
