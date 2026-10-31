//! Compare material noise samplers and surface depth with Java 26.3.
use anyhow::Result;
use native_resources::{
    density::WorldgenRandom,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    surface_rules::{
        SurfaceBands, SurfaceInputs, SurfaceNoise, SurfacePosition, SurfaceRandom,
        SurfaceRuleCompiler,
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command, sync::Arc};
struct Data(BTreeMap<String, Vec<u8>>);
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.get(path).cloned())
    }
}
struct Inputs<'a> {
    noise: &'a mut SurfaceNoise,
    bands: &'a SurfaceBands,
}
impl SurfaceInputs for Inputs<'_> {
    fn biome_matches(&mut self, _: &Value, _: &SurfacePosition) -> Result<bool> {
        anyhow::bail!("unexpected biome")
    }
    fn cold_enough_to_snow(&mut self, _: &SurfacePosition) -> Result<bool> {
        anyhow::bail!("unexpected temperature")
    }
    fn noise(&mut self, id: &ResourceId, is_3d: bool, p: &SurfacePosition) -> Result<f64> {
        Ok(f64::from(self.noise.sample(id, is_3d, p.x, p.y, p.z)?))
    }
    fn positional_random(&mut self, _: &ResourceId, _: &SurfacePosition) -> Result<f32> {
        anyhow::bail!("unexpected random")
    }
    fn ore_random(&mut self, _: &SurfacePosition) -> Result<SurfaceRandom> {
        anyhow::bail!("unexpected ore random")
    }
    fn density(&mut self, _: &Value, _: bool, _: &SurfacePosition) -> Result<f32> {
        anyhow::bail!("unexpected density")
    }
    fn bandlands(&mut self, p: &SurfacePosition) -> Result<Arc<Value>> {
        self.bands.at(self.noise, p.x, p.y, p.z)
    }
}
fn run(c: &mut Command) -> String {
    let out = c.output().expect("cannot run Java 25");
    assert!(
        out.status.success(),
        "surface noise oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn seeded_surface_noise_and_depth_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SurfaceNoiseOracle.java");
    std::fs::write(&source, include_str!("java/SurfaceNoiseOracle.java")).unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut stack = ResourceStack::open(&game).unwrap();
    let mut data = Data(BTreeMap::new());
    let mut definitions = serde_json::Map::new();
    let mut names = Vec::new();
    for name in [
        "minecraft:surface",
        "minecraft:surface_secondary",
        "minecraft:clay_bands_offset",
        "minecraft:patch",
        "minecraft:nether_state_selector",
        "minecraft:calcite",
        "minecraft:nether/temperature",
        "example:custom",
    ] {
        let id = ResourceId::parse(name).unwrap();
        let path = format!("data/{}/worldgen/noise/{}.json", id.namespace, id.path);
        let bytes = if id.namespace == "minecraft" {
            stack.read_builtin_data(&path).unwrap().unwrap()
        } else {
            serde_json::to_vec(
                &json!({"base_octave":-4,"octave_count":4,"amplitude_modifiers":[0,1,0,2]}),
            )
            .unwrap()
        };
        definitions.insert(name.into(), serde_json::from_slice(&bytes).unwrap());
        data.0.insert(path, bytes);
        names.push(id);
    }
    let input = temp.path().join("noises.json");
    std::fs::write(&input, serde_json::to_vec(&definitions).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SurfaceNoiseOracle")
        .arg(&input));
    let mut compiler = SurfaceRuleCompiler::new(&mut data);
    let mut rules = BTreeMap::new();
    for id in &names {
        for is_3d in [false, true] {
            let name = format!("{}:{}", id.namespace, id.path);
            let rule = json!({"type":"condition","if_true":{"type":"noise_threshold","noise":name,"min_threshold":-0.125,"max_threshold":0.125,"is_3d":is_3d},"then_run":{"type":"block","result_state":"minecraft:stone"}});
            rules.insert((id.clone(), is_3d), compiler.compile(&rule).unwrap());
        }
    }
    let band_rule = compiler.compile(&json!({"type":"bandlands"})).unwrap();
    assert!(
        band_rule
            .noise_dependencies()
            .contains(&ResourceId::parse("minecraft:clay_bands_offset").unwrap())
    );
    drop(compiler);
    let mut sampler = None;
    let mut bands = None;
    let (mut groups, mut samples, mut depths) = (0, 0, 0);
    let (mut band_slots, mut band_samples) = (0, 0);
    for line in output.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        match f.first().copied() {
            Some("S") => {
                let mode: i32 = f[1].parse().unwrap();
                let seed: i64 = f[2].parse().unwrap();
                let algorithm = if mode == 0 {
                    WorldgenRandom::Legacy
                } else {
                    WorldgenRandom::Xoroshiro
                };
                bands = Some(SurfaceBands::new(seed, algorithm));
                sampler =
                    Some(SurfaceNoise::load(&mut data, seed, algorithm, names.clone()).unwrap());
                groups += 1;
            }
            Some("P") => {
                let id = ResourceId::parse(f[1]).unwrap();
                let int = |i: usize| f[i].parse::<i32>().unwrap();
                let bits = |i: usize| f[i].parse::<i64>().unwrap() as u64;
                let s = sampler.as_mut().unwrap();
                assert_eq!(
                    f64::from(s.sample(&id, false, int(2), int(3), int(4)).unwrap()).to_bits(),
                    bits(5),
                    "2d {line}"
                );
                assert_eq!(
                    f64::from(s.sample(&id, true, int(2), int(3), int(4)).unwrap()).to_bits(),
                    bits(6),
                    "3d {line}"
                );
                let position = SurfacePosition {
                    x: int(2),
                    y: int(3),
                    z: int(4),
                    min_y: -64,
                    height: 384,
                    sea_level: 63,
                    stone_above: 0,
                    stone_below: 0,
                    surface_depth: 0,
                    surface_secondary: 0.0,
                    water_height: i32::MIN,
                    min_surface_level: 0,
                    gradient_x: 0,
                    gradient_z: 0,
                };
                for (is_3d, index) in [(false, 7), (true, 8)] {
                    let actual = rules[&(id.clone(), is_3d)]
                        .evaluate(
                            &position,
                            &mut Inputs {
                                noise: &mut *s,
                                bands: bands.as_ref().unwrap(),
                            },
                        )
                        .unwrap()
                        .is_some();
                    assert_eq!(actual, int(index) != 0, "noise threshold {line}");
                }
                samples += 1;
            }
            Some("B") => {
                let index: usize = f[1].parse().unwrap();
                assert_eq!(
                    bands.as_ref().unwrap().bands()[index].id(),
                    f[2],
                    "band {line}"
                );
                band_slots += 1;
            }
            Some("K") => {
                let x: i32 = f[1].parse().unwrap();
                let y: i32 = f[2].parse().unwrap();
                let z: i32 = f[3].parse().unwrap();
                let position = SurfacePosition {
                    x,
                    y,
                    z,
                    min_y: -64,
                    height: 384,
                    sea_level: 63,
                    stone_above: 0,
                    stone_below: 0,
                    surface_depth: 0,
                    surface_secondary: 0.0,
                    water_height: i32::MIN,
                    min_surface_level: 0,
                    gradient_x: 0,
                    gradient_z: 0,
                };
                let result = band_rule.evaluate(
                    &position,
                    &mut Inputs {
                        noise: sampler.as_mut().unwrap(),
                        bands: bands.as_ref().unwrap(),
                    },
                );
                if f[4] == "ERR" {
                    assert!(result.is_err(), "expected Java index error: {line}");
                } else {
                    let state = result.unwrap().unwrap();
                    assert_eq!(state.as_str().unwrap(), f[4], "bandlands {line}");
                }
                band_samples += 1;
            }
            Some("D") => {
                let x: i32 = f[1].parse().unwrap();
                let z: i32 = f[2].parse().unwrap();
                let s = sampler.as_mut().unwrap();
                assert_eq!(
                    s.surface_depth(x, z).unwrap(),
                    f[3].parse::<i32>().unwrap(),
                    "depth {line}"
                );
                assert_eq!(
                    s.surface_secondary(x, z).unwrap().to_bits(),
                    f[4].parse::<i64>().unwrap() as u64,
                    "secondary {line}"
                );
                depths += 1;
            }
            _ => {}
        }
    }
    assert_eq!(groups, 10);
    assert_eq!(samples, 10 * 14 * names.len());
    assert_eq!(depths, 10 * 14);
    assert_eq!(band_slots, 10 * 192);
    assert_eq!(band_samples, 10 * 14 * 13);
    assert!(
        sampler
            .unwrap()
            .sample(
                &ResourceId::parse("example:missing").unwrap(),
                false,
                0,
                0,
                0
            )
            .is_err()
    );
}
