use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::{
    biome_climate::{BiomeClimate, TemperatureModifier},
    freeze_feature::{FreezeWorld, freeze_top_layer},
};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

struct World {
    mode: i32,
    changes: BTreeMap<[i32; 3], String>,
    writes: Vec<String>,
}
impl World {
    fn original(p: [i32; 3]) -> String {
        if p[1] == 9 {
            return match (p[0] * 31 + p[2] * 17).rem_euclid(7) {
                0 => "water0",
                1 => "grassfalse",
                2 => "stone",
                3 => "air",
                4 => "water3",
                5 => "snow8",
                _ => "snow1",
            }
            .into();
        }
        if p[1] == 10 {
            match (p[0] * 7 + p[2] * 3).rem_euclid(13) {
                0 => return "snow2".into(),
                1 => return "stone".into(),
                _ => {}
            }
        }
        "air".into()
    }
    fn value(&self, p: [i32; 3]) -> String {
        self.changes
            .get(&p)
            .cloned()
            .unwrap_or_else(|| Self::original(p))
    }
    fn write(&mut self, p: [i32; 3], value: &str) {
        self.changes.insert(p, value.into());
        self.writes
            .push(format!("{},{},{},{}", p[0], p[1], p[2], value));
    }
}
impl FreezeWorld for World {
    type State = String;
    fn motion_height(&mut self, x: i32, z: i32) -> Result<i32> {
        Ok(if self.mode == 4 {
            if (x + z).rem_euclid(2) == 0 { 0 } else { 16 }
        } else {
            10
        })
    }
    fn biome(&mut self, p: [i32; 3]) -> Result<BiomeClimate> {
        let (temperature, has_precipitation) = match self.mode {
            0 => (0.0, true),
            1 => (0.0, false),
            2 => (0.8, true),
            _ => match (p[0] + p[2]).rem_euclid(3) {
                0 => (0.8, true),
                1 => (0.0, true),
                _ => (0.0, false),
            },
        };
        Ok(BiomeClimate {
            temperature,
            has_precipitation,
            modifier: TemperatureModifier::None,
        })
    }
    fn sea_level(&self) -> Result<i32> {
        Ok(63)
    }
    fn inside_build_height(&self, y: i32) -> bool {
        (0..16).contains(&y)
    }
    fn block_light(&mut self, p: [i32; 3]) -> Result<u8> {
        Ok(if (p[0] + 2 * p[2]).rem_euclid(5) == 0 {
            10
        } else {
            9
        })
    }
    fn block(&mut self, p: [i32; 3]) -> Result<String> {
        Ok(self.value(p))
    }
    fn freezable_water(&mut self, _: [i32; 3], state: &String) -> Result<bool> {
        Ok(state == "water0")
    }
    fn air_or_snow(&mut self, state: &String) -> Result<bool> {
        Ok(state == "air" || state.starts_with("snow"))
    }
    fn snow_survives(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(matches!(
            self.value([p[0], p[1] - 1, p[2]]).as_str(),
            "stone" | "grassfalse" | "grasstrue" | "ice" | "snow8"
        ))
    }
    fn put_ice(&mut self, p: [i32; 3]) -> Result<()> {
        self.write(p, "ice");
        Ok(())
    }
    fn put_snow(&mut self, p: [i32; 3]) -> Result<()> {
        self.write(p, "snow1");
        Ok(())
    }
    fn mark_snowy(&mut self, p: [i32; 3]) -> Result<()> {
        if self.value(p).starts_with("grass") {
            self.write(p, "grasstrue");
        }
        Ok(())
    }
}
fn run(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&o.stderr),
        String::from_utf8_lossy(&o.stdout)
    );
    String::from_utf8(o.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn freeze_top_layer_matches_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("FreezeFeatureOracle.java");
    std::fs::write(&source, include_str!("java/FreezeFeatureOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FreezeFeatureOracle"));
    let mut expected = Vec::new();
    for mode in 0..5 {
        let mut world = World {
            mode,
            changes: BTreeMap::new(),
            writes: vec![],
        };
        let mut random = DecorationRandom::new(123);
        let ok = freeze_top_layer(&mut world, [-8, 0, 3]).unwrap();
        expected.push(format!(
            "C {mode} {ok} W {} R {}",
            world.writes.join(";"),
            random.next_long()
        ));
    }
    let actual: Vec<_> = actual.lines().filter(|v| v.starts_with("C ")).collect();
    assert_eq!(actual.len(), 5);
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*a, b, "case {i}");
    }
}
