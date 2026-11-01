//! Compare buried-treasure world changes and loot RNG against Java 26.3.
use native_core::{
    beardifier::{BoundingBox, TerrainAdjustment},
    random::{DecorationRandom, LegacyRandom, XoroshiroRandom},
};
use native_resources::{
    structure_piece::{TreasureRandom, TreasureWorld, place_buried_treasure},
    structure_start::{HorizontalDirection, SimpleStructureKind, SimpleStructureStart},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};
fn run(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "Java treasure oracle failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}
fn vec3(v: &Value) -> [i32; 3] {
    [0, 1, 2].map(|i| v[i].as_i64().unwrap() as i32)
}
fn name(v: &Value) -> &str {
    v.as_str()
        .or_else(|| v.get("id").and_then(Value::as_str))
        .unwrap()
}
enum Random {
    Legacy(LegacyRandom),
    Xoroshiro(XoroshiroRandom),
    Decoration(DecorationRandom),
}
impl TreasureRandom for Random {
    fn next_long(&mut self) -> i64 {
        match self {
            Self::Legacy(r) => r.next_long(),
            Self::Xoroshiro(r) => r.next_long(),
            Self::Decoration(r) => r.next_long(),
        }
    }
}
struct World {
    blocks: BTreeMap<[i32; 3], Value>,
    writes: Vec<String>,
    entity: bool,
    loot: Option<([i32; 3], String, i64)>,
}
impl TreasureWorld for World {
    fn vertical_bounds(&self) -> (i32, i32) {
        (0, 20)
    }
    fn ocean_floor_height(&mut self, _: i32, _: i32) -> anyhow::Result<i32> {
        Ok(12)
    }
    fn block(&mut self, p: [i32; 3]) -> anyhow::Result<Value> {
        Ok(self
            .blocks
            .get(&p)
            .cloned()
            .unwrap_or_else(|| json!("minecraft:air")))
    }
    fn is_air(&mut self, s: &Value) -> anyhow::Result<bool> {
        Ok(matches!(
            name(s),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        ))
    }
    fn is_solid_render(&mut self, s: &Value) -> anyhow::Result<bool> {
        Ok(matches!(
            name(s),
            "minecraft:stone"
                | "minecraft:sandstone"
                | "minecraft:andesite"
                | "minecraft:granite"
                | "minecraft:diorite"
                | "minecraft:sand"
                | "minecraft:gravel"
                | "minecraft:dirt"
        ))
    }
    fn set_block_and_update(&mut self, p: [i32; 3], s: &Value) -> anyhow::Result<()> {
        self.blocks.insert(p, s.clone());
        let facing = if name(s) == "minecraft:chest" {
            s.get("properties")
                .and_then(|p| p.get("facing"))
                .and_then(Value::as_str)
                .unwrap_or("north")
        } else {
            "-"
        };
        self.writes.push(format!(
            "W {} {} {} {} {}",
            p[0],
            p[1],
            p[2],
            name(s),
            facing
        ));
        Ok(())
    }
    fn set_chest(&mut self, p: [i32; 3], facing: HorizontalDirection) -> anyhow::Result<bool> {
        let facing = format!("{facing:?}").to_lowercase();
        self.blocks.insert(
            p,
            json!({"id":"minecraft:chest","properties":{"facing":facing}}),
        );
        self.writes.push(format!(
            "W {} {} {} minecraft:chest {}",
            p[0], p[1], p[2], facing
        ));
        Ok(self.entity)
    }
    fn set_chest_loot(&mut self, p: [i32; 3], table: &str, seed: i64) -> anyhow::Result<()> {
        self.loot = Some((p, table.to_owned(), seed));
        Ok(())
    }
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn buried_treasure_placement_matches_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
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
    let source = temp.path().join("BuriedTreasureOracle.java");
    std::fs::write(&source, include_str!("java/BuriedTreasureOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let palette = [
        "air", "water", "lava", "sand", "gravel", "dirt", "stone", "chest",
    ];
    let support = [
        "stone",
        "sandstone",
        "andesite",
        "granite",
        "diorite",
        "dirt",
    ];
    let positions = [
        [9, 6, 9],
        [9, 7, 9],
        [9, 4, 9],
        [9, 6, 8],
        [10, 6, 9],
        [9, 6, 10],
        [8, 6, 9],
        [9, 5, 8],
        [10, 5, 9],
        [9, 5, 10],
        [8, 5, 9],
    ];
    let mut fixtures = Vec::new();
    let mut random = 3_886_719_021u64;
    for i in 0..180 {
        let mut blocks = Vec::new();
        blocks.push(json!({"position":[9,5,9],"state":format!("minecraft:{}",support[i%6])}));
        for p in positions {
            random = random
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let state = palette[((random >> 32) as usize) % palette.len()];
            blocks.push(json!({"position":p,"state":format!("minecraft:{state}")}));
        }
        let seed = [0, -1, 12345, i64::MIN, i64::MAX][i % 5];
        fixtures.push(json!({"id":i,"seed":seed,"mode":i%3,"entity":i%4!=0,"clip_min":[0,0,0],"clip_max":[if i%7==0 {8}else{15},20,15],"blocks":blocks}));
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&fixtures).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BuriedTreasureOracle")
        .arg(&input));
    let mut lines = output
        .lines()
        .filter(|l| {
            ["S ", "W ", "B ", "C ", "R "]
                .iter()
                .any(|p| l.starts_with(p))
        })
        .peekable();
    let mut no_foundation = 0;
    let mut no_chest = 0;
    let mut without_entity = 0;
    let mut with_loot = 0;
    for fixture in fixtures {
        let id = fixture["id"].as_i64().unwrap();
        assert_eq!(lines.next().unwrap(), format!("S {id}"));
        let mut expected = Vec::new();
        loop {
            let line = lines.next().unwrap();
            expected.push(line.to_owned());
            if line.starts_with("R ") {
                break;
            }
        }
        let blocks = fixture["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| (vec3(&b["position"]), b["state"].clone()))
            .collect();
        let mut world = World {
            blocks,
            writes: Vec::new(),
            entity: fixture["entity"].as_bool().unwrap(),
            loot: None,
        };
        let seed = fixture["seed"].as_i64().unwrap();
        let mut rng = match fixture["mode"].as_i64().unwrap() {
            0 => Random::Legacy(LegacyRandom::new(seed)),
            1 => Random::Xoroshiro(XoroshiroRandom::new(seed)),
            _ => Random::Decoration(DecorationRandom::new(seed)),
        };
        let p = [9, 90, 9];
        let mut piece = SimpleStructureStart {
            kind: SimpleStructureKind::BuriedTreasure,
            chunk: [0, 0],
            generation_point: [8, 10, 8],
            orientation: None,
            bounds: BoundingBox::new(p, p).unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        let clip =
            BoundingBox::new(vec3(&fixture["clip_min"]), vec3(&fixture["clip_max"])).unwrap();
        let outcome = place_buried_treasure(&mut piece, &mut world, clip, &mut rng).unwrap();
        match outcome {
            native_resources::structure_piece::BuriedTreasureOutcome::NoFoundation => {
                no_foundation += 1
            }
            native_resources::structure_piece::BuriedTreasureOutcome::Found {
                chest_placed: false,
                ..
            } => no_chest += 1,
            native_resources::structure_piece::BuriedTreasureOutcome::Found {
                chest_placed: true,
                loot_seed: None,
                ..
            } => without_entity += 1,
            native_resources::structure_piece::BuriedTreasureOutcome::Found {
                chest_placed: true,
                loot_seed: Some(_),
                ..
            } => with_loot += 1,
        }
        let mut actual = world.writes;
        let a = piece.bounds.min;
        let b = piece.bounds.max;
        actual.push(format!(
            "B {} {} {} {} {} {}",
            a[0], a[1], a[2], b[0], b[1], b[2]
        ));
        if let Some((p, table, seed)) = world.loot {
            actual.push(format!("C {} {} {} {} {}", p[0], p[1], p[2], table, seed));
        }
        actual.push(format!("R {}", rng.next_long()));
        assert_eq!(actual, expected, "fixture {id}");
    }
    assert!(lines.next().is_none());
    assert!(
        no_foundation > 0 && no_chest > 0 && without_entity > 0 && with_loot > 0,
        "fixture coverage: {no_foundation} {no_chest} {without_entity} {with_loot}"
    );
    let mut random_cases = 0;
    for line in output.lines().filter(|s| s.starts_with("D ")) {
        let words: Vec<_> = line.split_whitespace().collect();
        let seed = words[1].parse::<i64>().unwrap();
        let x = words[2].parse::<i32>().unwrap();
        let z = words[3].parse::<i32>().unwrap();
        let index = words[4].parse::<i32>().unwrap();
        let step = words[5].parse::<i32>().unwrap();
        let mut random = DecorationRandom::new(0);
        let decoration = random.set_decoration_seed(seed, x, z);
        assert_eq!(decoration, words[6].parse::<i64>().unwrap(), "{line}");
        random.set_feature_seed(decoration, index, step);
        assert_eq!(
            random.next_long(),
            words[7].parse::<i64>().unwrap(),
            "{line}"
        );
        assert_eq!(
            random.next_int(),
            words[8].parse::<i32>().unwrap(),
            "{line}"
        );
        assert_eq!(
            random.next_int_bound(1234567).unwrap(),
            words[9].parse::<i32>().unwrap(),
            "{line}"
        );
        assert_eq!(
            random.next_bool(),
            words[10].parse::<bool>().unwrap(),
            "{line}"
        );
        assert_eq!(
            random.next_float().to_bits(),
            words[11].parse::<i32>().unwrap() as u32,
            "{line}"
        );
        assert_eq!(
            random.next_double().to_bits(),
            words[12].parse::<i64>().unwrap() as u64,
            "{line}"
        );
        random_cases += 1;
    }
    assert_eq!(random_cases, 100);
}
