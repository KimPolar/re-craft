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

struct MoonFixture;
impl ResourceProvider for MoonFixture {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if path != "data/minecraft/cat_variant/black.json" {
            return Ok(None);
        }
        let mut entries = Vec::new();
        for (min, max) in [(0.0, 0.0), (0.25, 0.5), (0.75, 1.0), (0.9, 1.0)] {
            entries.push(json!({"priority":0,"condition":{"type":"minecraft:moon_brightness","range":{"min":min,"max":max}}}));
        }
        Ok(Some(serde_json::to_vec(
            &json!({"spawn_conditions":entries}),
        )?))
    }
    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        match folder {
            "cat_variant" => Ok(vec![ResourceId::parse("minecraft:black")?]),
            "cat_sound_variant" => Ok(vec![ResourceId::parse("minecraft:classic")?]),
            _ => anyhow::bail!("unknown registry"),
        }
    }
}
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
                for witch_first in [false, true] {
                    let algorithm = if legacy {
                        WorldgenRandom::Legacy
                    } else {
                        WorldgenRandom::Xoroshiro
                    };
                    let mut random = RegionRandom::new(seed, algorithm, -13, 17);
                    let witch_path = if witch_first {
                        let initialization =
                            native_resources::worldgen_region_random::finalize_mob_base(
                                &mut random,
                                false,
                            );
                        let tag = native_session::worldgen::draft_fresh_hut_witch(
                            [0, 3, 0, 4],
                            [3, 10, -3],
                            initialization,
                        )
                        .unwrap();
                        let path = temp.path().join(format!("witch-joint-{}.nbt", cases.len()));
                        std::fs::write(
                            &path,
                            write(&NamedTag {
                                name: "".into(),
                                value: tag,
                            })
                            .unwrap(),
                        )
                        .unwrap();
                        Some(path)
                    } else {
                        None
                    };
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
                    let mut case = json!({"mode":mode,"legacy":legacy,"seed":seed,"path":path,"next":random.next_long()});
                    if let Some(path) = witch_path {
                        case["witch_path"] = json!(path);
                    }
                    cases.push(case);
                }
            }
        }
    }
    let moons = CatSpawnVariants::load_active(&mut MoonFixture)
        .unwrap()
        .prepare_world_conditions(&mut MoonFixture)
        .unwrap();
    let mut checks = Vec::new();
    for phase in 0..8 {
        let context = native_resources::cat_variants::CatSpawnContext {
            biome: None,
            moon_phase: Some(phase),
            structures_with_piece: None,
        };
        for (min, max) in [(0.0, 0.0), (0.25, 0.5), (0.75, 1.0), (0.9, 1.0)] {
            let condition =
                json!({"type":"minecraft:moon_brightness","range":{"min":min,"max":max}});
            checks.push(json!({"phase":phase,"min":min,"max":max,"expected":moons.matches(&condition,&context).unwrap()}));
        }
    }
    cases[0]["moon_checks"] = json!(checks);
    let structure = ResourceId::parse("minecraft:swamp_hut").unwrap();
    let piece = native_core::beardifier::BoundingBox::new([3, 10, -3], [9, 16, 5]).unwrap();
    let mut checks = Vec::new();
    for x in [2, 3, 9, 10] {
        for y in [9, 10, 16, 17] {
            for z in [-4, -3, 5, 6] {
                let actual =
                    native_session::worldgen::structure_references::structures_with_piece_at(
                        [x, y, z],
                        [(&structure, std::slice::from_ref(&piece))],
                    )
                    .unwrap();
                checks.push(json!({"x":x,"y":y,"z":z,"expected":actual.contains(&structure)}));
            }
        }
    }
    cases[0]["piece_checks"] = json!(checks);
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let constructions = temp.path().join("constructions.json");
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("GeneratedHutCatOracle")
        .arg(input)
        .arg(&constructions));
    let observations: Value =
        serde_json::from_slice(&std::fs::read(constructions).unwrap()).unwrap();
    for case in observations.as_array().unwrap() {
        let id = ResourceId::parse(case["entity"].as_str().unwrap()).unwrap();
        let final_state = case["state"].as_u64().unwrap();
        let uuid: Vec<_> = case["uuid"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap() as i32)
            .collect();
        let mut initial = final_state;
        let mut matched = None;
        for _ in 0..32 {
            initial = initial.wrapping_sub(11).wrapping_mul(0xdfe05bcb1365) & ((1_u64 << 48) - 1);
            let random = native_core::random::LegacyRandom::from_state(initial).unwrap();
            let candidate =
                native_session::worldgen::FreshCreatureRandom::from_random(&id, random).unwrap();
            if candidate.construction.uuid.as_slice() == uuid
                && candidate.random.state() == final_state
            {
                matched = Some(candidate);
                break;
            }
        }
        let actual = matched.unwrap_or_else(|| panic!("constructor RNG mismatch for {}", id.path));
        assert_eq!(
            actual.initial_yaw.to_bits() as i32,
            case["yaw_bits"].as_i64().unwrap() as i32
        );
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3"]
fn installed_hut_cat_variants_use_the_structure_selector() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut provider = native_resources::packs::ResourceStack::open(&game).unwrap();
    let variants = CatSpawnVariants::load_active(&mut provider).unwrap();
    let conditions = variants.prepare_world_conditions(&mut provider).unwrap();
    let pieces =
        std::collections::BTreeSet::from([ResourceId::parse("minecraft:swamp_hut").unwrap()]);
    let context = native_resources::cat_variants::CatSpawnContext {
        biome: None,
        moon_phase: None,
        structures_with_piece: Some(&pieces),
    };
    let mut random = RegionRandom::new(42, WorldgenRandom::Xoroshiro, -13, 17);
    let cat = variants
        .finalize_hut_cat(&mut random, |condition| {
            conditions.matches(condition, &context)
        })
        .unwrap();
    assert_eq!(
        cat.variant,
        ResourceId::parse("minecraft:all_black").unwrap()
    );
}
