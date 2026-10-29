//! Sky-color attributes sampled at the camera's surrounding noise biomes.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use glam::DVec3;
use native_resources::packs::{ResourceId, ResourceProvider};
use native_world::loaded::World;
use serde_json::Value;

use crate::preview_lighting::{SkyTrack, rgb};

#[derive(Clone, Copy)]
struct ColorLayer {
    value: [f32; 3],
    multiply: bool,
}

impl ColorLayer {
    fn parse(value: &Value) -> Result<Self> {
        if value.is_string() || value.is_number() {
            return Ok(Self {
                value: rgb(value)?,
                multiply: false,
            });
        }
        let modifier = value
            .get("modifier")
            .and_then(Value::as_str)
            .context("missing color modifier")?;
        let id = ResourceId::parse(modifier)?;
        ensure!(id.namespace == "minecraft", "unsupported color modifier");
        let multiply = match id.path.as_str() {
            "override" => false,
            "multiply" => true,
            _ => bail!("unsupported color modifier {modifier}"),
        };
        Ok(Self {
            value: rgb(value.get("argument").context("missing color argument")?)?,
            multiply,
        })
    }

    fn apply(self, base: [f32; 3]) -> [f32; 3] {
        if self.multiply {
            std::array::from_fn(|i| base[i] * self.value[i])
        } else {
            self.value
        }
    }
}

fn find_color_attribute(attributes: &Value, attribute: &str) -> Result<Option<ColorLayer>> {
    let entries = attributes
        .as_object()
        .context("invalid environment attributes")?;
    let mut result: Option<ColorLayer> = None;
    for (name, value) in entries {
        let id = ResourceId::parse(name)?;
        if id.namespace == "minecraft" && id.path == attribute {
            ensure!(result.is_none(), "duplicate {attribute} attribute");
            result = Some(ColorLayer::parse(value)?);
        }
    }
    Ok(result)
}

#[derive(Clone, Copy)]
enum FloatModifier {
    Override,
    Add,
    Subtract,
    Multiply,
    Minimum,
    Maximum,
}

#[derive(Clone, Copy)]
struct FloatLayer {
    value: f32,
    modifier: FloatModifier,
}

struct FogTrack {
    index: usize,
    track: SkyTrack,
    modifier: FloatModifier,
}

impl FogTrack {
    fn parse(index: usize, timeline: &Value, value: &Value) -> Result<Self> {
        let modifier = match value.get("modifier") {
            None => FloatModifier::Override,
            Some(name) => {
                FloatLayer::parse(&serde_json::json!({"modifier": name, "argument": 1.0}))?.modifier
            }
        };
        let mut normalized = value.clone();
        normalized
            .as_object_mut()
            .context("invalid fog timeline")?
            .insert("modifier".into(), Value::String("override".into()));
        Ok(Self {
            index,
            track: SkyTrack::parse(timeline, &normalized, true)?,
            modifier,
        })
    }
}

impl FloatLayer {
    fn parse(value: &Value) -> Result<Self> {
        let (modifier, argument) = if value.is_number() {
            (FloatModifier::Override, value)
        } else {
            let name = value
                .get("modifier")
                .and_then(Value::as_str)
                .context("missing float modifier")?;
            let id = ResourceId::parse(name)?;
            ensure!(id.namespace == "minecraft", "unsupported float modifier");
            let modifier = match id.path.as_str() {
                "override" => FloatModifier::Override,
                "add" => FloatModifier::Add,
                "subtract" => FloatModifier::Subtract,
                "multiply" => FloatModifier::Multiply,
                "minimum" => FloatModifier::Minimum,
                "maximum" => FloatModifier::Maximum,
                _ => bail!("unsupported float modifier {name}"),
            };
            (
                modifier,
                value.get("argument").context("missing float argument")?,
            )
        };
        let value = argument.as_f64().context("invalid float argument")? as f32;
        ensure!(value.is_finite(), "non-finite float argument");
        Ok(Self { value, modifier })
    }
    fn apply(self, base: f32) -> f32 {
        match self.modifier {
            FloatModifier::Override => self.value,
            FloatModifier::Add => base + self.value,
            FloatModifier::Subtract => base - self.value,
            FloatModifier::Multiply => base * self.value,
            FloatModifier::Minimum => base.min(self.value),
            FloatModifier::Maximum => base.max(self.value),
        }
    }
}

fn fog_layers(attributes: Option<&Value>) -> Result<[Option<FloatLayer>; 2]> {
    let mut result = [None; 2];
    let Some(attributes) = attributes else {
        return Ok(result);
    };
    for (name, value) in attributes
        .as_object()
        .context("invalid environment attributes")?
    {
        let id = ResourceId::parse(name)?;
        if id.namespace != "minecraft" {
            continue;
        }
        let index = match id.path.as_str() {
            "visual/fog_start_distance" => 0,
            "visual/fog_end_distance" => 1,
            _ => continue,
        };
        ensure!(result[index].is_none(), "duplicate fog distance");
        result[index] = Some(FloatLayer::parse(value)?);
    }
    Ok(result)
}

pub struct SkyBackground {
    base: [f32; 3],
    biomes: BTreeMap<String, Option<ColorLayer>>,
    tracks: Vec<SkyTrack>,
    fog_base: Option<[f32; 2]>,
    fog_biomes: BTreeMap<String, Option<[Option<FloatLayer>; 2]>>,
    fog_tracks: Option<Vec<FogTrack>>,
    min_y: i32,
    max_y: i32,
}

impl SkyBackground {
    pub fn load(
        source: &mut impl ResourceProvider,
        dimension_type: &str,
        min_y: i32,
        max_y: i32,
    ) -> Result<Option<Self>> {
        ensure!(min_y < max_y, "invalid dimension bounds");
        let id = ResourceId::parse(dimension_type)?;
        let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
        let bytes = source
            .read_data(&path)?
            .with_context(|| format!("missing {path}"))?;
        let dimension: Value =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid {path}"))?;
        let skybox = match dimension.get("skybox") {
            None => "overworld",
            Some(value) => value.as_str().context("invalid skybox")?,
        };
        let attribute = match skybox {
            "overworld" => "visual/sky_color",
            // Without a skybox, atmospheric fog is the background.
            "none" => "visual/fog_color",
            "end" => return Ok(None),
            _ => bail!("unsupported skybox"),
        };
        let base = match dimension.get("attributes") {
            None => [0.0; 3],
            Some(attributes) => find_color_attribute(attributes, attribute)?
                .map_or([0.0; 3], |layer| layer.apply([0.0; 3])),
        };
        let fog_base = if skybox == "none" {
            fog_layers(dimension.get("attributes")).ok().map(|layers| {
                let defaults = [0.0, 1024.0];
                std::array::from_fn(|i| {
                    layers[i].map_or(defaults[i], |layer| layer.apply(defaults[i]))
                })
            })
        } else {
            None
        };
        let mut fog_biomes = BTreeMap::new();
        let mut biomes = BTreeMap::new();
        for id in source.list_data("worldgen/biome")? {
            let path = format!("data/{}/worldgen/biome/{}.json", id.namespace, id.path);
            let bytes = source
                .read_data(&path)?
                .with_context(|| format!("missing {path}"))?;
            let biome: Value =
                serde_json::from_slice(&bytes).with_context(|| format!("invalid {path}"))?;
            let layer = biome
                .get("attributes")
                .map(|attributes| find_color_attribute(attributes, attribute))
                .transpose()?
                .flatten();
            let name = format!("{}:{}", id.namespace, id.path);
            fog_biomes.insert(name.clone(), fog_layers(biome.get("attributes")).ok());
            ensure!(biomes.insert(name, layer).is_none(), "duplicate biome");
        }
        ensure!(!biomes.is_empty(), "biome registry is empty");
        let mut tracks = Vec::new();
        let mut fog_tracks = Some(Vec::new());
        for timeline in
            native_resources::fluid_environment::load_dimension_timelines(source, dimension_type)?
        {
            let entries = timeline
                .get("tracks")
                .and_then(Value::as_object)
                .context("invalid timeline tracks")?;
            let mut found = false;
            for (name, value) in entries {
                let id = ResourceId::parse(name)?;
                if id.namespace == "minecraft"
                    && let Some(index) = match id.path.as_str() {
                        "visual/fog_start_distance" => Some(0),
                        "visual/fog_end_distance" => Some(1),
                        _ => None,
                    }
                {
                    match FogTrack::parse(index, &timeline, value) {
                        Ok(track) => {
                            if let Some(tracks) = &mut fog_tracks {
                                tracks.push(track);
                            }
                        }
                        Err(_) => fog_tracks = None,
                    }
                }
                if id.namespace == "minecraft" && id.path == attribute {
                    ensure!(!found, "duplicate {attribute} timeline");
                    found = true;
                    tracks.push(SkyTrack::parse(&timeline, value, false)?);
                }
            }
        }
        Ok(Some(Self {
            base,
            biomes,
            tracks,
            fog_base,
            fog_biomes,
            fog_tracks,
            min_y,
            max_y,
        }))
    }

    pub fn sample(
        &self,
        world: &World,
        position: DVec3,
        mut clock_ticks: impl FnMut(&ResourceId) -> Option<i64>,
    ) -> Option<([f32; 3], Option<[f32; 2]>)> {
        let min_y = self.min_y.div_euclid(4) * 4;
        let max_y = (self.max_y - 1).div_euclid(4) * 4;
        let grouped = group_biomes(position, |x, y, z| {
            world.biome(
                x.checked_mul(4)?,
                y.checked_mul(4)?.clamp(min_y, max_y),
                z.checked_mul(4)?,
            )
        })?;
        let mut value = interpolate_color(&grouped, &self.biomes, self.base)?;
        for track in &self.tracks {
            let arg = track.sample(clock_ticks(&track.clock)?);
            value = if track.multiply {
                std::array::from_fn(|i| value[i] * arg[i])
            } else {
                arg
            };
        }
        let fog_range = (|| {
            let mut range = interpolate_fog(&grouped, &self.fog_biomes, self.fog_base?)?;
            for track in self.fog_tracks.as_ref()? {
                let arg = track.track.sample(clock_ticks(&track.track.clock)?)[0];
                range[track.index] = FloatLayer {
                    value: arg,
                    modifier: track.modifier,
                }
                .apply(range[track.index]);
            }
            range[1] = range[1].max(0.0);
            range.iter().all(|n| n.is_finite()).then_some(range)
        })();
        Some((value, fog_range))
    }
}

fn group_biomes<'a>(
    position: DVec3,
    mut biome: impl FnMut(i32, i32, i32) -> Option<&'a str>,
) -> Option<Vec<(&'a str, f64)>> {
    if !position.is_finite() {
        return None;
    }
    let p = position * 0.25 - DVec3::splat(0.5);
    if p.min_element() < f64::from(i32::MIN + 2) || p.max_element() > f64::from(i32::MAX - 3) {
        return None;
    }
    let floor = p.floor();
    let frac = p - floor;
    let cell = floor.as_ivec3();
    let kernel = [0.0, 1.0, 4.0, 6.0, 4.0, 1.0, 0.0];
    let weight = |i: usize, f: f64| kernel[i + 1] + f * (kernel[i] - kernel[i + 1]);
    let mut grouped: Vec<(&str, f64)> = Vec::new();
    for z in 0..6 {
        for x in 0..6 {
            for y in 0..6 {
                let id = biome(
                    cell.x - 2 + x as i32,
                    cell.y - 2 + y as i32,
                    cell.z - 2 + z as i32,
                )?;
                let w = weight(x, frac.x) * weight(y, frac.y) * weight(z, frac.z);
                if let Some((_, total)) = grouped.iter_mut().find(|(key, _)| *key == id) {
                    *total += w;
                } else {
                    grouped.push((id, w));
                }
            }
        }
    }
    Some(grouped)
}

fn interpolate_color(
    grouped: &[(&str, f64)],
    layers: &BTreeMap<String, Option<ColorLayer>>,
    base: [f32; 3],
) -> Option<[f32; 3]> {
    let mut total = 0.0;
    let mut result: Option<[f32; 3]> = None;
    for &(id, weight) in grouped {
        let color = layers.get(id)?.map_or(base, |layer| layer.apply(base));
        total += weight;
        result = Some(match result {
            None => color,
            Some(current) => {
                let t = (weight / total) as f32;
                std::array::from_fn(|i| current[i] + t * (color[i] - current[i]))
            }
        });
    }
    result
}

fn interpolate_fog(
    grouped: &[(&str, f64)],
    layers: &BTreeMap<String, Option<[Option<FloatLayer>; 2]>>,
    base: [f32; 2],
) -> Option<[f32; 2]> {
    let mut total = 0.0;
    let mut result: Option<[f32; 2]> = None;
    for &(id, weight) in grouped {
        let layers = layers.get(id)?.as_ref()?;
        let value: [f32; 2] =
            std::array::from_fn(|i| layers[i].map_or(base[i], |layer| layer.apply(base[i])));
        total += weight;
        result = Some(match result {
            None => value,
            Some(current) => {
                let t = (weight / total) as f32;
                std::array::from_fn(|i| current[i] + t * (value[i] - current[i]))
            }
        });
    }
    result
}

#[cfg(test)]
fn gaussian_biomes<'a>(
    position: DVec3,
    biome: impl FnMut(i32, i32, i32) -> Option<&'a str>,
    layers: &BTreeMap<String, Option<ColorLayer>>,
    base: [f32; 3],
) -> Option<[f32; 3]> {
    interpolate_color(&group_biomes(position, biome)?, layers, base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn blended_sky_color_matches_java_sampler() {
        use native_resources::packs::ResourceStack;
        use serde_json::json;
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
        let track = &day["tracks"]["minecraft:visual/sky_color"];
        let biomes = [
            json!({"minecraft:visual/sky_color":"#abcdef","minecraft:visual/fog_color":"#330808","minecraft:visual/fog_start_distance":{"modifier":"add","argument":5.0},"minecraft:visual/fog_end_distance":50.0}),
            json!({"minecraft:visual/sky_color":{"modifier":"multiply","argument":"#7295ff"},"minecraft:visual/fog_color":"#1b4745","minecraft:visual/fog_start_distance":-10.0,"minecraft:visual/fog_end_distance":{"modifier":"multiply","argument":0.5}}),
            json!({}),
        ];
        let mut cases = Vec::new();
        for p in [
            [0.0, 64.0, 0.0],
            [-0.01, -20.0, 7.75],
            [123.125, 77.5, -456.25],
            [29999990.5, 120.25, -29999990.5],
        ] {
            for tick in [
                -1, 0, 730, 6000, 11270, 12000, 13140, 18000, 22860, 23999, 24000,
            ] {
                cases.push(json!({"position":p,"tick":tick}));
            }
        }
        let fog_track = json!({"modifier":"multiply","keyframes":[{"ticks":0,"value":1.0},{"ticks":12000,"value":0.5}]});
        let input = json!({"biomes":biomes,"track":track,"fogTrack":fog_track,"cases":cases});
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("SkyBackgroundOracle.java");
        let input_file = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/SkyBackgroundOracle.java"),
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
            .arg("SkyBackgroundOracle")
            .arg(&input_file)
            .arg(&output));
        let expected: Vec<Vec<f32>> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let layers = BTreeMap::from([
            (
                "a".into(),
                find_color_attribute(&biomes[0], "visual/sky_color").unwrap(),
            ),
            (
                "b".into(),
                find_color_attribute(&biomes[1], "visual/sky_color").unwrap(),
            ),
            ("c".into(), None),
        ]);
        let fog_color_layers = BTreeMap::from([
            (
                "a".into(),
                find_color_attribute(&biomes[0], "visual/fog_color").unwrap(),
            ),
            (
                "b".into(),
                find_color_attribute(&biomes[1], "visual/fog_color").unwrap(),
            ),
            ("c".into(), None),
        ]);
        let sky_track = SkyTrack::parse(&day, track, false).unwrap();
        let fog_distance_track = SkyTrack::parse(&day, &fog_track, true).unwrap();
        let fog_distance_layers = BTreeMap::from([
            ("a".into(), Some(fog_layers(Some(&biomes[0])).unwrap())),
            ("b".into(), Some(fog_layers(Some(&biomes[1])).unwrap())),
            ("c".into(), Some([None; 2])),
        ]);
        assert_eq!(expected.len(), cases.len() + 6);
        for (case, expected) in cases.iter().zip(&expected) {
            let p = &case["position"];
            let position = DVec3::new(
                p[0].as_f64().unwrap(),
                p[1].as_f64().unwrap(),
                p[2].as_f64().unwrap(),
            );
            let mut actual = gaussian_biomes(
                position,
                |x, y, z| Some(["a", "b", "c"][(x + 3 * y + 7 * z).rem_euclid(3) as usize]),
                &layers,
                [120.0 / 255.0, 167.0 / 255.0, 1.0],
            )
            .unwrap();
            let factor = sky_track.sample(case["tick"].as_i64().unwrap());
            let fog = gaussian_biomes(
                position,
                |x, y, z| Some(["a", "b", "c"][(x + 3 * y + 7 * z).rem_euclid(3) as usize]),
                &fog_color_layers,
                [0.0; 3],
            )
            .unwrap();
            let grouped = group_biomes(position, |x, y, z| {
                Some(["a", "b", "c"][(x + 3 * y + 7 * z).rem_euclid(3) as usize])
            })
            .unwrap();
            let mut distance =
                interpolate_fog(&grouped, &fog_distance_layers, [10.0, 96.0]).unwrap();
            distance[1] *= fog_distance_track.sample(case["tick"].as_i64().unwrap())[0];
            for i in 0..2 {
                assert!(
                    (distance[i] - expected[i + 6]).abs() < 0.0001,
                    "distance case {case}, channel {i}: {} != {}",
                    distance[i],
                    expected[i + 6]
                );
            }
            for i in 0..3 {
                actual[i] *= factor[i];
                assert!(
                    (actual[i] - expected[i]).abs() < 0.000002,
                    "case {case}, channel {i}: {} != {}",
                    actual[i],
                    expected[i]
                );
                assert!(
                    (fog[i] - expected[i + 3]).abs() < 0.000002,
                    "fog case {case}, channel {i}: {} != {}",
                    fog[i],
                    expected[i + 3]
                );
            }
        }
        for (name, expected) in [
            "override", "add", "subtract", "multiply", "minimum", "maximum",
        ]
        .into_iter()
        .zip(&expected[cases.len()..])
        {
            let layer = FloatLayer::parse(&json!({"modifier":name,"argument":3.0})).unwrap();
            assert_eq!(layer.apply(10.0), expected[0], "modifier {name}");
        }
        assert!(
            SkyBackground::load(&mut resources, "minecraft:overworld", -64, 320)
                .unwrap()
                .is_some()
        );
        let nether = SkyBackground::load(&mut resources, "minecraft:the_nether", 0, 128)
            .unwrap()
            .unwrap();
        assert_eq!(nether.fog_base, Some([10.0, 96.0]));
        for (biome, expected) in [
            ("minecraft:nether_wastes", [0x33, 0x08, 0x08]),
            ("minecraft:soul_sand_valley", [0x1b, 0x47, 0x45]),
        ] {
            let layer = nether.biomes.get(biome).unwrap().unwrap();
            assert!(!layer.multiply);
            assert_eq!(layer.value, expected.map(|channel| channel as f32 / 255.0));
        }
        assert!(
            SkyBackground::load(&mut resources, "minecraft:the_end", 0, 256)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn gaussian_preserves_uniform_biome_and_requires_loaded_samples() {
        let layers = BTreeMap::from([(
            "minecraft:plains".into(),
            Some(ColorLayer {
                value: [0.2, 0.4, 0.6],
                multiply: false,
            }),
        )]);
        for p in [DVec3::ZERO, DVec3::new(-100.25, 64.0, 33.75)] {
            assert_eq!(
                gaussian_biomes(p, |_, _, _| Some("minecraft:plains"), &layers, [1.0; 3]),
                Some([0.2, 0.4, 0.6])
            );
        }
        assert!(
            gaussian_biomes(
                DVec3::ZERO,
                |x, _, _| (x != 0).then_some("minecraft:plains"),
                &layers,
                [1.0; 3]
            )
            .is_none()
        );
    }

    #[test]
    fn skyless_dimension_loads_fog_attributes_and_matching_timeline() {
        use serde_json::json;
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/example/dimension_type/test.json" => json!({
                        "skybox":"none", "timelines":["example:day"],
                        "attributes":{"visual/fog_color":"#ffffff","visual/sky_color":"#ff0000","visual/fog_start_distance":10.0,"visual/fog_end_distance":96.0}
                    }),
                    "data/example/worldgen/biome/test.json" => json!({"attributes":{
                        "visual/fog_color":"#204060", "visual/sky_color":"#00ff00",
                        "visual/fog_start_distance":{"modifier":"add","argument":5.0}, "visual/fog_end_distance":64.0
                    }}),
                    "data/example/timeline/day.json" => json!({
                        "clock":"example:clock", "period_ticks":20, "tracks":{
                            "visual/fog_color":{"modifier":"multiply","keyframes":[{"ticks":0,"value":"#ffffff"},{"ticks":10,"value":"#000000"}]},
                            "visual/fog_end_distance":{"modifier":"add","keyframes":[{"ticks":0,"value":0.0},{"ticks":10,"value":16.0}]},
                            "visual/sky_color":{"keyframes":[{"ticks":0,"value":"#ff0000"}]}
                        }
                    }),
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
        let background = SkyBackground::load(&mut Data, "example:test", 0, 16)
            .unwrap()
            .unwrap();
        assert_eq!(background.base, [1.0; 3]);
        assert_eq!(background.fog_base, Some([10.0, 96.0]));
        let groups = group_biomes(DVec3::ZERO, |_, _, _| Some("example:test")).unwrap();
        let mut range = interpolate_fog(
            &groups,
            &background.fog_biomes,
            background.fog_base.unwrap(),
        )
        .unwrap();
        assert_eq!(range, [15.0, 64.0]);
        let track = &background.fog_tracks.as_ref().unwrap()[0];
        range[track.index] = FloatLayer {
            value: track.track.sample(10)[0],
            modifier: track.modifier,
        }
        .apply(range[track.index]);
        assert_eq!(range, [15.0, 80.0]);
        let fog = gaussian_biomes(
            DVec3::ZERO,
            |_, _, _| Some("example:test"),
            &background.biomes,
            background.base,
        )
        .unwrap();
        assert_eq!(fog, [32.0 / 255.0, 64.0 / 255.0, 96.0 / 255.0]);
        assert_eq!(background.tracks.len(), 1);
        assert!(background.tracks[0].multiply);
        assert_eq!(background.tracks[0].sample(10), [0.0; 3]);
    }

    #[test]
    fn samples_loaded_world_and_time_without_fabricating_missing_biomes() {
        use native_world::loaded::{BIOME_COUNT, Chunk, ChunkPos, Section, World};
        use serde_json::json;
        use std::sync::Arc;
        let mut world = World::new("minecraft:overworld");
        let plains = world.intern_biome("minecraft:plains".into());
        for x in -1..=1 {
            for z in -1..=1 {
                let position = ChunkPos { x, z };
                world.chunks.insert(
                    position,
                    Arc::new(Chunk {
                        position,
                        status: None,
                        sections: BTreeMap::from([(
                            0,
                            Arc::new(Section {
                                blocks: None,
                                biomes: Some(Box::new([plains; BIOME_COUNT])),
                                block_light: None,
                                sky_light: None,
                            }),
                        )]),
                    }),
                );
            }
        }
        let timeline = json!({"clock":"example:clock","period_ticks":20});
        let track = json!({"modifier":"multiply","keyframes":[{"ticks":0,"value":"#ffffff"},{"ticks":10,"value":"#000000"}]});
        let mut background = SkyBackground {
            base: [0.0; 3],
            biomes: BTreeMap::from([(
                "minecraft:plains".into(),
                Some(ColorLayer {
                    value: [0.2, 0.4, 0.6],
                    multiply: false,
                }),
            )]),
            tracks: vec![SkyTrack::parse(&timeline, &track, false).unwrap()],
            fog_base: None,
            fog_biomes: BTreeMap::from([("minecraft:plains".into(), Some([None; 2]))]),
            fog_tracks: Some(Vec::new()),
            min_y: 0,
            max_y: 16,
        };
        assert_eq!(
            background.sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0)),
            Some(([0.2, 0.4, 0.6], None))
        );
        assert_eq!(
            background.sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(10)),
            Some(([0.0; 3], None))
        );
        assert!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| None)
                .is_none()
        );
        background.fog_base = Some([10.0, 96.0]);
        assert_eq!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .unwrap()
                .1,
            Some([10.0, 96.0])
        );
        background.fog_biomes.insert(
            "minecraft:plains".into(),
            Some([
                Some(FloatLayer {
                    value: 5.0,
                    modifier: FloatModifier::Add,
                }),
                Some(FloatLayer {
                    value: 0.5,
                    modifier: FloatModifier::Multiply,
                }),
            ]),
        );
        background.fog_tracks = Some(vec![
            FogTrack::parse(
                1,
                &timeline,
                &json!({"modifier":"add","keyframes":[{"ticks":0,"value":8.0}]}),
            )
            .unwrap(),
        ]);
        assert_eq!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .unwrap()
                .1,
            Some([15.0, 56.0])
        );
        background
            .fog_biomes
            .insert("minecraft:plains".into(), None);
        assert_eq!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .unwrap()
                .1,
            None
        );
        world.chunks.remove(&ChunkPos { x: -1, z: -1 });
        assert!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .is_none()
        );
    }
}
