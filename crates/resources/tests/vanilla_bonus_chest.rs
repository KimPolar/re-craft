use anyhow::Result;
use native_core::random::{DecorationRandom, LegacyRandom, XoroshiroRandom};
use native_resources::bonus_chest::{
    BonusChestFeature, BonusChestRandom, BonusChestWorld, place_bonus_chest,
};
use std::{path::PathBuf, process::Command};

#[derive(Default)]
struct World {
    mode: i32,
    has_entity: bool,
    probes: Vec<String>,
    writes: Vec<String>,
    loot: Option<([i32; 3], i64)>,
}

#[test]
fn bonus_chest_feature_uses_active_data_pack_definition() {
    use native_resources::packs::ResourceProvider;
    struct Data(Option<Vec<u8>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            assert_eq!(path, "data/minecraft/worldgen/feature/bonus_chest.json");
            Ok(self.0.clone())
        }
    }
    assert!(BonusChestFeature::load(&mut Data(None)).unwrap().is_none());
    assert!(
        BonusChestFeature::load(&mut Data(Some(
            br#"{"type":"minecraft:bonus_chest"}"#.to_vec()
        )))
        .unwrap()
        .is_some()
    );
    assert!(
        BonusChestFeature::load(&mut Data(Some(br#"{"type":"example:other"}"#.to_vec()))).is_err()
    );
    assert!(BonusChestFeature::load(&mut Data(Some(br#"{"type":false}"#.to_vec()))).is_err());
    assert!(BonusChestFeature::load(&mut Data(Some(b"{".to_vec()))).is_err());
}

fn fmt_pos(p: [i32; 3]) -> String {
    format!("{},{},{}", p[0], p[1], p[2])
}

impl BonusChestWorld for World {
    fn motion_blocking_no_leaves(&mut self, x: i32, z: i32) -> Result<[i32; 3]> {
        self.probes.push(format!("{x},{z}"));
        Ok([x, 8, z])
    }
    fn is_empty_block(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(self.mode == 3 || self.mode == 1 && p[0] & 15 == 7 && p[2] & 15 == 3)
    }
    fn collision_shape_empty(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(self.mode == 2 && (p[0] ^ p[2]) & 3 == 0)
    }
    fn set_chest(&mut self, p: [i32; 3]) -> Result<()> {
        self.writes.push(format!("{},chest", fmt_pos(p)));
        Ok(())
    }
    fn has_randomizable_container(&mut self, _: [i32; 3]) -> Result<bool> {
        Ok(self.has_entity)
    }
    fn set_bonus_chest_loot(&mut self, p: [i32; 3], seed: i64) -> Result<()> {
        self.loot = Some((p, seed));
        Ok(())
    }
    fn torch_can_survive(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(p[0].wrapping_mul(31).wrapping_add(p[2].wrapping_mul(17)) & 3 != 0)
    }
    fn set_torch(&mut self, p: [i32; 3]) -> Result<()> {
        self.writes.push(format!("{},torch", fmt_pos(p)));
        Ok(())
    }
}

#[test]
fn blocked_columns_leave_no_writes_and_consume_only_shuffles() {
    let mut world = World::default();
    let mut random = LegacyRandom::new(42);
    let mut expected = random.clone();
    for _ in 0..2 {
        for bound in (2..=16).rev() {
            expected.next_int_bound(bound).unwrap();
        }
    }
    assert_eq!(
        place_bonus_chest(&mut world, &mut random, [-17, 90, -33]).unwrap(),
        None
    );
    assert_eq!(world.probes.len(), 256);
    assert!(world.writes.is_empty());
    assert_eq!(random.next_long(), expected.next_long());
}

fn render_case<R: BonusChestRandom>(
    kind: i32,
    seed: i64,
    mode: i32,
    has: bool,
    random: &mut R,
) -> String {
    let origins = [[0, 20, 0], [-17, -50, -33], [i32::MAX, 8, i32::MIN]];
    let origin = origins[((mode + kind) % 3) as usize];
    let mut world = World {
        mode,
        has_entity: has,
        ..Default::default()
    };
    let placed = place_bonus_chest(&mut world, random, origin)
        .unwrap()
        .is_some();
    let loot = world
        .loot
        .map_or_else(|| "-".to_owned(), |(p, s)| format!("{},{s}", fmt_pos(p)));
    format!(
        "C {kind} {seed} {mode} {has} {placed} P {} W {} L {loot} R {}",
        world.probes.join(";"),
        world.writes.join(";"),
        random.loot_seed()
    )
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn bonus_chest_matches_java_placement_and_random_state() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    assert!(BonusChestFeature::load(&mut resources).unwrap().is_some());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_owned()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("BonusChestOracle.java");
    std::fs::write(&source, include_str!("java/BonusChestOracle.java")).unwrap();
    let run = |cmd: &mut Command| {
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("BonusChestOracle"));
    let actual: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("C "))
        .collect();
    let mut expected = Vec::new();
    for kind in 0..3 {
        for seed in [0, 1, -1, 987654321, i64::MIN, i64::MAX] {
            for mode in 0..4 {
                for has in [false, true] {
                    expected.push(match kind {
                        0 => render_case(kind, seed, mode, has, &mut LegacyRandom::new(seed)),
                        1 => render_case(kind, seed, mode, has, &mut XoroshiroRandom::new(seed)),
                        _ => render_case(kind, seed, mode, has, &mut DecorationRandom::new(seed)),
                    });
                }
            }
        }
    }
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*a, b, "case {i}");
    }
}
