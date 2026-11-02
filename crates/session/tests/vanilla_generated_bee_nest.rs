//! Compare generated bee-nest NBT and its Java roundtrip with 26.3.
use native_persistence::nbt::{self, NamedTag, Tag};
use native_session::worldgen::GeneratedBeeNest;
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

fn run(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "Java bee nest oracle failed: {}",
        String::from_utf8_lossy(&o.stderr)
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
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn generated_bee_nests_roundtrip_java_nbt() {
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
    let source = temp.path().join("GeneratedBeeNestOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedBeeNestOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    assert!(
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GeneratedBeeNestOracle")
            .arg("java")
            .arg(temp.path()))
        .contains("OK")
    );
    for (i, ticks) in [
        vec![0, 598],
        vec![1, 2, 3],
        vec![598, 0, 300],
        vec![400, 598],
    ]
    .into_iter()
    .enumerate()
    {
        let pos = [-17 + i as i32, 63 - i as i32, 31 + i as i32];
        let nest = GeneratedBeeNest::new(pos, ticks).unwrap();
        let expected =
            nbt::read(&std::fs::read(temp.path().join(format!("java-{i}.nbt"))).unwrap()).unwrap();
        assert_eq!(
            normalized(&nest.block_entity()),
            normalized(&expected.value),
            "case {i}"
        );
        std::fs::write(
            temp.path().join(format!("rust-{i}.nbt")),
            nbt::write(&NamedTag {
                name: "".into(),
                value: nest.block_entity(),
            })
            .unwrap(),
        )
        .unwrap();
    }
    assert!(
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GeneratedBeeNestOracle")
            .arg("rust")
            .arg(temp.path()))
        .contains("OK")
    );
}
