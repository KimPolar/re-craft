use native_resources::{
    feature_order::{BiomeFeatures, FeatureKey, FeatureSchedule, FeatureTags, order_features},
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
};

type Case = Vec<Vec<Vec<i32>>>;

fn run(command: &mut Command) -> String {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "Java feature order failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn feature_order_matches_java_for_biomes_and_cycles() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("FeatureOrderOracle.java");
    std::fs::write(&source, include_str!("java/FeatureOrderOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));

    let mut cases: Vec<Case> = vec![
        vec![],
        vec![vec![vec![], vec![]]],
        vec![vec![vec![0, 1], vec![2]], vec![vec![0, 3, 1], vec![2, 4]]],
        vec![vec![vec![0, 1]], vec![vec![1, 0]]],
        vec![vec![vec![0, 0]]],
        vec![vec![vec![0], vec![0]], vec![vec![], vec![1]]],
    ];
    let mut seed = 0x18c3d5a9u64;
    let mut random = |limit: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 32) as usize % limit
    };
    for _ in 0..240 {
        let mut biomes = Vec::new();
        for _ in 0..random(5) {
            let mut steps = Vec::new();
            for _ in 0..random(5) {
                let mut features = Vec::new();
                for _ in 0..random(5) {
                    features.push(random(8) as i32);
                }
                steps.push(features);
            }
            biomes.push(steps);
        }
        cases.push(biomes);
    }
    let mut provider = ResourceStack::open(&game).unwrap();
    let registry: BTreeSet<_> = provider
        .list_data("worldgen/placed_feature")
        .unwrap()
        .into_iter()
        .collect();
    assert!(registry.len() > 100);
    let mut tags = FeatureTags::default();
    let mut keys = BTreeMap::<FeatureKey, i32>::new();
    let mut actual_biomes = Vec::new();
    let mut possible_biomes = Vec::new();
    for name in [
        "plains",
        "desert",
        "forest",
        "snowy_plains",
        "mushroom_fields",
        "deep_dark",
    ] {
        let id = ResourceId::parse(&format!("minecraft:{name}")).unwrap();
        possible_biomes.push(id.clone());
        let features = BiomeFeatures::load(&mut provider, &id, &registry, &mut tags).unwrap();
        let mut steps = Vec::new();
        for features in features.steps {
            let mut ids = Vec::new();
            for feature in features {
                let next = keys.len() as i32;
                ids.push(*keys.entry(feature).or_insert(next));
            }
            steps.push(ids);
        }
        actual_biomes.push(steps);
    }
    let schedule = FeatureSchedule::load(&mut provider, &possible_biomes, &registry).unwrap();
    let scheduled: Vec<Vec<i32>> = schedule
        .steps()
        .iter()
        .map(|step| step.iter().map(|key| keys[key]).collect())
        .collect();
    assert_eq!(scheduled, order_features(&actual_biomes).unwrap());
    cases.push(actual_biomes);
    let input = temp.path().join("feature_order_cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java_output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FeatureOrderOracle")
        .arg(input));
    let expected: Vec<Value> = serde_json::from_str(java_output.lines().last().unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (i, (case, expected)) in cases.iter().zip(expected).enumerate() {
        let actual = match order_features(case) {
            Ok(steps) => json!(steps),
            Err(_) => json!("cycle"),
        };
        assert_eq!(actual, expected, "feature order case {i}");
    }
}
