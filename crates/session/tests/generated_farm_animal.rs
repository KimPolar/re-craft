use std::{path::PathBuf, process::Command};

use native_persistence::nbt::{NamedTag, write};
use native_resources::{
    animal_variants::{FarmAnimal, FarmAnimalSpawnVariants},
    density::WorldgenRandom,
    packs::{ResourceId, ResourceStack},
    worldgen_region_random::{AnimalGroupData, RegionRandom},
};
use native_session::worldgen::{FreshCreatureRandom, draft_fresh_farm_animal};
use serde_json::{Value, json};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn fresh_farm_animal_drafts_match_java_entities() {
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
    let source = temp.path().join("GeneratedFarmAnimalOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedFarmAnimalOracle.java")).unwrap();
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
        .arg(source));
    let mut resources = ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    for (name, species) in [
        ("pig", FarmAnimal::Pig),
        ("cow", FarmAnimal::Cow),
        ("chicken", FarmAnimal::Chicken),
    ] {
        let variants = FarmAnimalSpawnVariants::load_active(&mut resources, species).unwrap();
        for biome in [
            "minecraft:plains",
            "minecraft:desert",
            "minecraft:snowy_plains",
        ] {
            for legacy in [true, false] {
                for seed in [0_i64, 42, -1] {
                    for chance in [0.05_f32, 1.0] {
                        let algorithm = if legacy {
                            WorldgenRandom::Legacy
                        } else {
                            WorldgenRandom::Xoroshiro
                        };
                        let mut random = RegionRandom::new(seed, algorithm, -13, 17);
                        let mut group = AnimalGroupData {
                            baby_spawn_chance: chance,
                            ..Default::default()
                        };
                        let mut documents = Vec::new();
                        for index in 0..3 {
                            let init = variants
                                .finalize(
                                    &ResourceId::parse(biome).unwrap(),
                                    &mut random,
                                    &mut group,
                                    false,
                                )
                                .unwrap();
                            let egg = (species == FarmAnimal::Chicken)
                                .then_some([6000, 8999, 11999][index]);
                            let tag = draft_fresh_farm_animal(
                                species,
                                [0, 1, 0, 2],
                                [3.5, 10.0, -2.5],
                                22.5,
                                variants.default_variant(),
                                &init,
                                egg,
                            )
                            .unwrap();
                            let path = temp
                                .path()
                                .join(format!("animal-{}-{index}.nbt", cases.len()));
                            std::fs::write(
                                &path,
                                write(&NamedTag {
                                    name: "".into(),
                                    value: tag,
                                })
                                .unwrap(),
                            )
                            .unwrap();
                            documents.push(path);
                        }
                        cases.push(json!({"animal":name,"biome":biome,"legacy":legacy,"seed":seed,"chance":chance,"paths":documents}));
                    }
                }
            }
        }
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let constructions = temp.path().join("constructions.json");
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("GeneratedFarmAnimalOracle")
        .arg(input)
        .arg(&constructions));
    let construction_cases: Value =
        serde_json::from_slice(&std::fs::read(constructions).unwrap()).unwrap();
    for case in construction_cases.as_array().unwrap() {
        let name = case["entity"].as_str().unwrap();
        let id = ResourceId::parse(name).unwrap();
        let final_state = case["state"].as_u64().unwrap();
        let uuid: Vec<i32> = case["uuid"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap() as i32)
            .collect();
        let mut initial = final_state;
        let mut matched = None;
        // nextInt(6000) can reject a draw; account for that rare Java path.
        for _ in 0..32 {
            initial = initial.wrapping_sub(11).wrapping_mul(0xdfe05bcb1365) & ((1_u64 << 48) - 1);
            let random = native_core::random::LegacyRandom::from_state(initial).unwrap();
            let candidate = FreshCreatureRandom::from_random(&id, random).unwrap();
            if candidate.construction.uuid.as_slice() == uuid
                && candidate.random.state() == final_state
            {
                matched = Some(candidate);
                break;
            }
        }
        let actual = matched.unwrap_or_else(|| panic!("constructor RNG mismatch for {name}"));
        assert_eq!(
            actual.initial_yaw.to_bits() as i32,
            case["yaw_bits"].as_i64().unwrap() as i32,
            "{name}"
        );
        assert_eq!(
            actual.construction.chicken_egg_time.map(i64::from),
            case["egg"].as_i64(),
            "{name}"
        );
        assert_eq!(actual.random.state(), final_state, "{name}");
    }
}
