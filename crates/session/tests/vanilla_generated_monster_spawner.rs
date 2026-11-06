use native_persistence::nbt::{self, NamedTag, Tag};
use native_resources::monster_room::RoomMob;
use native_session::worldgen::{GeneratedCaveSpiderSpawner, GeneratedMonsterSpawner};
use std::{collections::BTreeMap, path::PathBuf, process::Command};
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
fn normalized(tag: &Tag) -> String {
    match tag {
        Tag::Compound(fields) => {
            let fields: BTreeMap<_, _> = fields
                .iter()
                .map(|(k, v)| (k.0.clone(), normalized(v)))
                .collect();
            format!("{fields:?}")
        }
        Tag::List {
            element_type,
            values,
        } => format!(
            "{element_type:?} {:?}",
            values.iter().map(normalized).collect::<Vec<_>>()
        ),
        _ => format!("{tag:?}"),
    }
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn generated_monster_spawners_roundtrip_java_nbt() {
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
    let source = temp.path().join("GeneratedMonsterSpawnerOracle.java");
    std::fs::write(
        &source,
        include_str!("java/GeneratedMonsterSpawnerOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let java_output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("GeneratedMonsterSpawnerOracle")
        .arg("java")
        .arg(temp.path()));
    assert!(java_output.contains("OK"));
    let expected_random = native_core::random::DecorationRandom::new(1).next_long();
    for i in 0..4 {
        assert!(
            java_output
                .lines()
                .any(|line| line == format!("R {i} {expected_random}")),
            "spawner consumed random values: {java_output}"
        );
        let position = [-17 + i as i32, 63 - i as i32, 31 + i as i32];
        let tag = match i {
            0 => GeneratedMonsterSpawner {
                position,
                mob: RoomMob::Skeleton,
            }
            .block_entity(),
            1 => GeneratedMonsterSpawner {
                position,
                mob: RoomMob::Zombie,
            }
            .block_entity(),
            2 => GeneratedMonsterSpawner {
                position,
                mob: RoomMob::Spider,
            }
            .block_entity(),
            _ => GeneratedCaveSpiderSpawner { position }.block_entity(),
        };
        let expected =
            nbt::read(&std::fs::read(temp.path().join(format!("java-{i}.nbt"))).unwrap()).unwrap();
        assert_eq!(normalized(&tag), normalized(&expected.value), "case {i}");
        std::fs::write(
            temp.path().join(format!("rust-{i}.nbt")),
            nbt::write(&NamedTag {
                name: "".into(),
                value: tag,
            })
            .unwrap(),
        )
        .unwrap();
    }
    assert!(
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GeneratedMonsterSpawnerOracle")
            .arg("rust")
            .arg(temp.path()))
        .contains("OK")
    );
}
