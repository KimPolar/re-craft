use std::{path::PathBuf, process::Command};

use native_persistence::nbt::{NamedTag, write};
use native_resources::{
    density::WorldgenRandom,
    worldgen_region_random::{
        DonkeyGroupData, DonkeyInitialization, HorseGroupData, HorseInitialization, RegionRandom,
        finalize_donkey, finalize_horse,
    },
};
use native_session::worldgen::{FreshCreatureRandom, draft_fresh_donkey, draft_fresh_horse};
use serde_json::{Value, json};

enum EquineInitialization {
    Horse(HorseInitialization),
    Donkey(DonkeyInitialization),
}

#[test]
fn horse_group_matches_java_seed_42() {
    let mut random = RegionRandom::new(42, WorldgenRandom::Legacy, -13, 17);
    let mut group = HorseGroupData::default();
    let expected = [
        (0, 29.0, false),
        (0, 16.0, false),
        (3, 20.0, false),
        (0, 24.0, false),
        (3, 20.0, true),
        (0, 24.0, false),
    ];
    for (index, (markings, health, baby)) in expected.into_iter().enumerate() {
        let horse = finalize_horse(&mut random, &mut group, false);
        assert_eq!((horse.variant, horse.markings), (1, markings));
        assert_eq!(horse.max_health, health);
        assert_eq!(horse.animal.set_baby_age, baby);
        if index == 0 {
            assert_eq!(horse.movement_speed.to_bits(), 4598568313994495078);
            assert_eq!(horse.jump_strength.to_bits(), 4604934550957544716);
        }
    }
    assert_eq!(random.next_long(), -403792309887560671);
}

#[test]
fn donkey_group_matches_java_seed_42() {
    let mut random = RegionRandom::new(42, WorldgenRandom::Legacy, -13, 17);
    let mut group = DonkeyGroupData::default();
    for health in [27.0, 24.0, 21.0, 21.0, 27.0, 21.0] {
        let donkey = finalize_donkey(&mut random, &mut group, false);
        assert_eq!(donkey.max_health, health);
        assert!(!donkey.animal.set_baby_age);
    }
    assert_eq!(random.next_long(), 1087479121136914901);
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn fresh_horse_groups_and_drafts_match_java() {
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
    let source = temp.path().join("GeneratedHorseOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedHorseOracle.java")).unwrap();
    let run = |command: &mut Command| {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let mut cases = Vec::new();
    let mut expected = Vec::new();
    for species in ["horse", "donkey"] {
        for legacy in [true, false] {
            for seed in [0_i64, 1, 42, -1, i64::MIN, i64::MAX] {
                let algorithm = if legacy {
                    WorldgenRandom::Legacy
                } else {
                    WorldgenRandom::Xoroshiro
                };
                let mut random = RegionRandom::new(seed, algorithm, -13, 17);
                let mut group = HorseGroupData::default();
                let mut donkey_group = DonkeyGroupData::default();
                let mut docs = Vec::new();
                let mut animals = Vec::new();
                for index in 0..6 {
                    let (tag, init) = if species == "horse" {
                        let init = finalize_horse(&mut random, &mut group, false);
                        (
                            draft_fresh_horse([0, 1, 0, 2], [3.5, 10.0, -2.5], 22.5, init).unwrap(),
                            EquineInitialization::Horse(init),
                        )
                    } else {
                        let init = finalize_donkey(&mut random, &mut donkey_group, false);
                        (
                            draft_fresh_donkey([0, 1, 0, 2], [3.5, 10.0, -2.5], 22.5, init)
                                .unwrap(),
                            EquineInitialization::Donkey(init),
                        )
                    };
                    let path = temp
                        .path()
                        .join(format!("horse-{}-{index}.nbt", cases.len()));
                    std::fs::write(
                        &path,
                        write(&NamedTag {
                            name: "".into(),
                            value: tag,
                        })
                        .unwrap(),
                    )
                    .unwrap();
                    docs.push(path);
                    animals.push(init);
                }
                cases.push(json!({"legacy":legacy,"seed":seed,"paths":docs,"species":species}));
                expected.push((animals, random.next_long()));
            }
        }
    }
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let constructors = temp.path().join("constructors.json");
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("GeneratedHorseOracle")
        .arg(&input)
        .arg(&output)
        .arg(&constructors));
    let actual: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    for (((animals, next), result), case) in
        expected.iter().zip(actual.as_array().unwrap()).zip(&cases)
    {
        let id =
            native_resources::packs::ResourceId::parse(case["species"].as_str().unwrap()).unwrap();
        let dimensions =
            native_resources::worldgen_mob_geometry::known_creature_spawn_dimensions(&id).unwrap();
        assert_eq!(dimensions.width, result["width"].as_f64().unwrap() as f32);
        assert_eq!(dimensions.height, result["height"].as_f64().unwrap() as f32);
        assert_eq!(*next, result["next"].as_i64().unwrap());
        for (equine, saved) in animals.iter().zip(result["animals"].as_array().unwrap()) {
            let (health, animal, speed, jump) = match equine {
                EquineInitialization::Horse(horse) => {
                    assert_eq!(u64::from(horse.variant), saved["variant"].as_u64().unwrap());
                    assert_eq!(
                        u64::from(horse.markings),
                        saved["markings"].as_u64().unwrap()
                    );
                    (
                        horse.max_health,
                        horse.animal,
                        horse.movement_speed,
                        horse.jump_strength,
                    )
                }
                EquineInitialization::Donkey(donkey) => {
                    (donkey.max_health, donkey.animal, f64::from(0.175_f32), 0.5)
                }
            };
            assert_eq!(
                health.to_bits() as i32 as i64,
                saved["health_bits"].as_i64().unwrap()
            );
            assert_eq!(
                speed.to_bits() as i64,
                saved["speed_bits"].as_i64().unwrap()
            );
            assert_eq!(jump.to_bits() as i64, saved["jump_bits"].as_i64().unwrap());
            assert_eq!(animal.set_baby_age, saved["baby"].as_bool().unwrap());
            assert_eq!(animal.left_handed, saved["left"].as_bool().unwrap());
        }
    }
    let constructors: Value =
        serde_json::from_slice(&std::fs::read(constructors).unwrap()).unwrap();
    for case in constructors.as_array().unwrap() {
        let final_state = case["state"].as_u64().unwrap();
        let uuid: Vec<i32> = case["uuid"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap() as i32)
            .collect();
        let mut state = final_state;
        let mut found = None;
        for _ in 0..32 {
            state = state.wrapping_sub(11).wrapping_mul(0xdfe05bcb1365) & ((1_u64 << 48) - 1);
            let random = native_core::random::LegacyRandom::from_state(state).unwrap();
            let horse = FreshCreatureRandom::from_random(
                &native_resources::packs::ResourceId::parse(case["species"].as_str().unwrap())
                    .unwrap(),
                random,
            )
            .unwrap();
            if horse.construction.uuid.as_slice() == uuid && horse.random.state() == final_state {
                found = Some(horse);
                break;
            }
        }
        let horse = found.expect("horse constructor RNG mismatch");
        assert_eq!(
            horse.initial_yaw.to_bits() as i32 as i64,
            case["yaw_bits"].as_i64().unwrap()
        );
        assert_eq!(horse.construction.chicken_egg_time, None);
    }
}
