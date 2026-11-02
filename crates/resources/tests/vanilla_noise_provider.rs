use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::{
    disk_feature::DiskWorld,
    ore_feature::OreWorld,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    simple_block_feature::{SimpleBlockFeature, SimpleBlockWorld},
};
use std::{path::PathBuf, process::Command};
#[derive(Default)]
struct World {
    selected: Option<String>,
}
impl OreWorld for World {
    type State = String;
    fn block(&mut self, _: [i32; 3]) -> Result<String> {
        panic!("noise provider should not query blocks")
    }
    fn block_is(&mut self, _: &String, _: &ResourceId) -> Result<bool> {
        panic!()
    }
    fn in_tag(&mut self, _: &String, _: &ResourceId) -> Result<bool> {
        panic!()
    }
    fn is_air(&mut self, _: &String) -> Result<bool> {
        panic!()
    }
    fn set_block(&mut self, _: [i32; 3], _: &String, _: i32) -> Result<bool> {
        panic!()
    }
}
impl DiskWorld for World {
    fn is_solid(&mut self, _: &String) -> Result<bool> {
        panic!()
    }
    fn fluid_is(&mut self, _: &String, _: &ResourceId) -> Result<bool> {
        panic!()
    }
    fn set_disk_block(&mut self, _: [i32; 3], _: &String) -> Result<bool> {
        panic!()
    }
    fn mark_postprocessing(&mut self, _: [i32; 3]) -> Result<()> {
        panic!()
    }
}
impl SimpleBlockWorld for World {
    fn place_simple_state(&mut self, _: [i32; 3], s: &String) -> Result<bool> {
        self.selected = Some(s.clone());
        Ok(true)
    }
    fn schedule_simple_tick(&mut self, _: [i32; 3]) -> Result<()> {
        panic!()
    }
}
fn run(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}

#[test]
fn named_noise_provider_uses_active_pack_and_validates_state_lists() {
    struct Data {
        low: &'static str,
        invalid: bool,
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            if path != "data/test/worldgen/block_state_provider/flowers.json" {
                return Ok(None);
            }
            let low = if self.invalid { vec![] } else { vec![self.low] };
            Ok(Some(serde_json::to_vec(&serde_json::json!({
                "type":"minecraft:noise_threshold","seed":2345,"noise":{"base_octave":0},"scale":0.005,
                "threshold":1.0,"high_chance":0.5,"default_state":"minecraft:dandelion",
                "low_states":low,"high_states":["minecraft:poppy"]
            }))?))
        }
    }
    let value = serde_json::json!({"type":"minecraft:simple_block","to_place":"test:flowers"});
    let mut resolve = |v: &serde_json::Value| Ok(v.as_str().unwrap().to_string());
    for chosen in ["minecraft:red_tulip", "minecraft:white_tulip"] {
        let feature = SimpleBlockFeature::decode_with_resources(
            &mut Data {
                low: chosen,
                invalid: false,
            },
            &value,
            &mut resolve,
        )
        .unwrap();
        assert!(
            feature
                .possible_states()
                .iter()
                .any(|s| s.as_str() == chosen)
        );
        let mut random = DecorationRandom::new(9);
        let mut reference = random.clone();
        let mut world = World::default();
        assert!(feature.place(&mut world, &mut random, [0, 64, 0]).unwrap());
        assert_eq!(world.selected.as_deref(), Some(chosen));
        let _ = reference.next_int_bound(1);
        assert_eq!(random.next_long(), reference.next_long());
    }
    assert!(
        SimpleBlockFeature::decode_with_resources(
            &mut Data {
                low: "minecraft:poppy",
                invalid: true
            },
            &value,
            &mut resolve
        )
        .is_err()
    );
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn installed_flower_noise_provider_matches_java() {
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
    let source = temp.path().join("NoiseThresholdOracle.java");
    std::fs::write(&source, include_str!("java/NoiseThresholdOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("NoiseThresholdOracle"));
    let mut resources = ResourceStack::open(&game).unwrap();
    let feature = SimpleBlockFeature::load(
        &mut resources,
        &ResourceId::parse("minecraft:flower_plain").unwrap(),
        &mut |v| Ok(v.as_str().or_else(|| v["id"].as_str()).unwrap().to_string()),
    )
    .unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let mut count = 0;
    for line in actual.lines().filter(|v| v.starts_with("C ")) {
        let a = line.split_whitespace().collect::<Vec<_>>();
        let p = [
            a[1].parse().unwrap(),
            a[2].parse().unwrap(),
            a[3].parse().unwrap(),
        ];
        let seed = a[4].parse().unwrap();
        let mut r = DecorationRandom::new(seed);
        let mut world = World::default();
        assert!(feature.place(&mut world, &mut r, p).unwrap());
        let selected = world.selected.unwrap();
        assert_eq!(selected, a[5], "{p:?} {seed}");
        assert_eq!(r.next_long(), a[6].parse::<i64>().unwrap(), "{p:?} {seed}");
        seen.insert(selected);
        count += 1;
    }
    assert!(
        count > 2000
            && seen.contains("minecraft:dandelion")
            && [
                "minecraft:poppy",
                "minecraft:azure_bluet",
                "minecraft:oxeye_daisy",
                "minecraft:cornflower"
            ]
            .iter()
            .any(|s| seen.contains(*s))
            && [
                "minecraft:orange_tulip",
                "minecraft:red_tulip",
                "minecraft:pink_tulip",
                "minecraft:white_tulip"
            ]
            .iter()
            .any(|s| seen.contains(*s)),
        "cases {count}, states {seen:?}"
    );
}
