//! Compare and round-trip fresh mineshaft chest-minecart NBT with Java 26.3.
use native_persistence::nbt::{self, NamedTag, Tag};
use native_session::worldgen::draft_fresh_mineshaft_chest_minecart;
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
#[ignore = "requires installed Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn generated_minecarts_roundtrip_java_nbt() {
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
                paths.push(p);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("GeneratedMinecartOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedMinecartOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("GeneratedMinecartOracle")
        .arg(temp.path())
        .arg("java"));
    for experimental in 0..2 {
        for (i, seed) in [0, 1, -1, i64::MIN].into_iter().enumerate() {
            let pos = [-17.5 + i as f64, 63.5 - i as f64, 31.5 + i as f64];
            let tag = draft_fresh_mineshaft_chest_minecart([0, 1, 0, 2], pos, seed).unwrap();
            let expected = nbt::read(
                &std::fs::read(temp.path().join(format!("minecart-{experimental}-{i}.nbt")))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                normalized(&tag),
                normalized(&expected.value),
                "experimental {experimental}, seed {seed}"
            );
            std::fs::write(
                temp.path().join(format!("rust-{experimental}-{i}.nbt")),
                nbt::write(&NamedTag {
                    name: "".into(),
                    value: tag,
                })
                .unwrap(),
            )
            .unwrap();
        }
    }
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("GeneratedMinecartOracle")
        .arg(temp.path())
        .arg("rust"));
}
