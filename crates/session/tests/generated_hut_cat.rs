use anyhow::Result;
use native_persistence::nbt::{NamedTag, write};
use native_resources::{
    cat_variants::CatSpawnVariants,
    density::WorldgenRandom,
    packs::{ResourceId, ResourceProvider},
    worldgen_region_random::RegionRandom,
};
use native_session::worldgen::draft_fresh_hut_cat;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

struct Fixture(&'static str);
impl ResourceProvider for Fixture {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if !path.starts_with("data/minecraft/cat_variant/") {
            return Ok(None);
        }
        let priority = i32::from(self.0 == "high" && path.ends_with("all_black.json"));
        let conditions = if self.0 == "none" {
            json!([])
        } else {
            json!([{ "priority": priority }])
        };
        Ok(Some(serde_json::to_vec(
            &json!({"spawn_conditions": conditions}),
        )?))
    }
    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        let names = match folder {
            "cat_variant" => ["all_black", "black"],
            "cat_sound_variant" => ["classic", "royal"],
            _ => anyhow::bail!("unknown registry"),
        };
        names
            .into_iter()
            .map(|id| ResourceId::parse(&format!("minecraft:{id}")))
            .collect()
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn fresh_hut_cats_match_java_nbt_and_region_random() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
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
    let source = temp.path().join("GeneratedHutCatOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedHutCatOracle.java")).unwrap();
    let run = |command: &mut Command| {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let mut cases = Vec::new();
    for mode in ["high", "tie", "none"] {
        let variants = CatSpawnVariants::load_active(&mut Fixture(mode)).unwrap();
        for legacy in [false, true] {
            for seed in [0_i64, 1, 42, -1, i64::MIN, i64::MAX] {
                let algorithm = if legacy {
                    WorldgenRandom::Legacy
                } else {
                    WorldgenRandom::Xoroshiro
                };
                let mut random = RegionRandom::new(seed, algorithm, -13, 17);
                let init = variants
                    .finalize_hut_cat(&mut random, |_| unreachable!("unconditional selectors"))
                    .unwrap();
                let tag = draft_fresh_hut_cat([0, 1, 0, 2], [3, 10, -3], &init).unwrap();
                let path = temp.path().join(format!("cat-{}.nbt", cases.len()));
                std::fs::write(
                    &path,
                    write(&NamedTag {
                        name: "".into(),
                        value: tag,
                    })
                    .unwrap(),
                )
                .unwrap();
                cases.push(json!({"mode":mode,"legacy":legacy,"seed":seed,"path":path,"next":random.next_long()}));
            }
        }
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("GeneratedHutCatOracle")
        .arg(input));
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3"]
fn installed_hut_cat_variants_use_the_structure_selector() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut provider = native_resources::packs::ResourceStack::open(&game).unwrap();
    let variants = CatSpawnVariants::load_active(&mut provider).unwrap();
    let mut random = RegionRandom::new(42, WorldgenRandom::Xoroshiro, -13, 17);
    let cat = variants
        .finalize_hut_cat(&mut random, |condition| {
            assert_eq!(condition["type"], "minecraft:structure");
            assert_eq!(condition["structures"], "#minecraft:cats_spawn_as_black");
            Ok(true)
        })
        .unwrap();
    assert_eq!(
        cat.variant,
        ResourceId::parse("minecraft:all_black").unwrap()
    );
}
