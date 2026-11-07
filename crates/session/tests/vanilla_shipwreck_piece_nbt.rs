use anyhow::{Context, Result, ensure};
use native_persistence::nbt::{self, Tag};
use native_session::worldgen::shipwreck::SavedShipwreckPiece;
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn canonical(tag: Tag) -> Tag {
    match tag {
        Tag::Compound(fields) => {
            let mut fields: Vec<_> = fields.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            fields.sort_by(|a, b| a.0.0.cmp(&b.0.0));
            Tag::Compound(fields)
        }
        Tag::List {
            element_type,
            values,
        } => Tag::List {
            element_type,
            values: values.into_iter().map(canonical).collect(),
        },
        other => other,
    }
}

#[test]
fn rejects_unpreservable_shipwreck_piece_data() -> Result<()> {
    let piece = native_resources::shipwreck::Piece {
        template: "minecraft:shipwreck/with_mast".into(),
        position: [-51, 40, 79],
        rotation: native_resources::structure_transform::Rotation::None,
        beached: true,
    };
    let bounds = piece.bounds([33, 8, 13])?;
    let tag = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:shipwreck".into())),
        (
            "BB".into(),
            Tag::IntArray(bounds.min.into_iter().chain(bounds.max).collect()),
        ),
        ("O".into(), Tag::Int(2)),
        ("GD".into(), Tag::Int(0)),
        ("TPX".into(), Tag::Int(-51)),
        ("TPY".into(), Tag::Int(40)),
        ("TPZ".into(), Tag::Int(79)),
        (
            "Template".into(),
            Tag::String("minecraft:shipwreck/with_mast".into()),
        ),
        ("Rot".into(), Tag::String("NONE".into())),
        ("isBeached".into(), Tag::Byte(1)),
        ("height_adjusted".into(), Tag::Byte(1)),
    ]);
    assert!(SavedShipwreckPiece::from_tag(&tag, [33, 8, 13]).is_ok());
    let mut unknown = tag.clone();
    if let Tag::Compound(ref mut fields) = unknown {
        fields.push(("unknown".into(), Tag::Int(1)));
    }
    assert!(SavedShipwreckPiece::from_tag(&unknown, [33, 8, 13]).is_err());
    assert!(SavedShipwreckPiece::from_tag(&tag, [1, 1, 1]).is_err());
    Ok(())
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn shipwreck_piece_nbt_and_reload_match_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong version");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"]
        .as_array()
        .context("missing libraries")?
    {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths)?;
    let source = temp.path().join("ShipwreckPieceNbtOracle.java");
    std::fs::write(&source, include_str!("java/ShipwreckPieceNbtOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("ShipwreckPieceNbtOracle")
        .arg(temp.path())
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut changed = 0;
    for i in 0..32 {
        let initial = nbt::read(&std::fs::read(temp.path().join(format!("piece-{i}.nbt")))?)?.value;
        let java_loaded = nbt::read(&std::fs::read(
            temp.path().join(format!("reloaded-{i}.nbt")),
        )?)?
        .value;
        let saved = SavedShipwreckPiece::from_tag(
            &initial,
            if i & 1 != 0 { [33, 8, 13] } else { [7, 4, 5] },
        )?;
        std::fs::write(
            temp.path().join(format!("rust-{i}.nbt")),
            nbt::write(&nbt::NamedTag {
                name: "".into(),
                value: saved.to_tag()?,
            })?,
        )?;
        assert_eq!(
            canonical(saved.to_tag()?),
            canonical(initial.clone()),
            "case {i}"
        );
        let loaded = saved.after_java_reload()?.to_tag()?;
        changed += usize::from(canonical(initial) != canonical(loaded.clone()));
        assert_eq!(canonical(loaded), canonical(java_loaded), "reload case {i}");
    }
    assert!(changed > 0, "test must cover the stale bounding box");
    let verified = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("ShipwreckPieceNbtOracle")
        .arg(temp.path())
        .arg("verify")
        .output()?;
    ensure!(
        verified.status.success(),
        "Java reload rejected Rust data: {}\n{}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    Ok(())
}
