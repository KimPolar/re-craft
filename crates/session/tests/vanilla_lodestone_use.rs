//! Compare compass use with Java's actual CompassItem and Inventory.
use glam::IVec3;
use native_persistence::nbt::Tag;
use native_resources::packs::ResourceId;
use native_session::{
    inventory::{ItemStack, PlayerInventory, PlayerSlot},
    lodestone::Tracker,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn tracker(position: IVec3) -> Tracker {
    Tracker::linked(ResourceId::parse("minecraft:overworld").unwrap(), position)
}
fn compass(count: u32, target: Option<&Tracker>) -> ItemStack {
    let components = target
        .map(|value| vec![("minecraft:lodestone_tracker".into(), value.to_nbt())])
        .unwrap_or_default();
    ItemStack::new(
        "minecraft:compass".into(),
        count,
        64,
        Tag::Compound(components),
    )
    .unwrap()
}
fn describe(stack: Option<&ItemStack>, target: &Tracker) -> String {
    let Some(stack) = stack else {
        return "-".into();
    };
    if stack.id() == "minecraft:stone" {
        return format!("s{}", stack.count());
    }
    assert_eq!(stack.id(), "minecraft:compass");
    let state = match stack.components().get("minecraft:lodestone_tracker") {
        None => "none",
        Some(value) if Tracker::parse(value).as_ref() == Some(target) => "new",
        Some(_) => "old",
    };
    format!("c{}{state}", stack.count())
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn compass_use_matches_java_inventory() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    native_resources::packs::ResourceStack::open(&game).unwrap();
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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("LodestoneUseOracle.java");
    let output = temp.path().join("uses.json");
    std::fs::write(&source, include_str!("java/LodestoneUseOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("LodestoneUseOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 64);
    let target = tracker(IVec3::new(-20, 64, 47));
    let old = tracker(IVec3::new(1, 2, 3));
    for row in rows {
        let boolean = |key: &str| row[key].as_bool().unwrap();
        let mut inventory = PlayerInventory::default();
        if boolean("full") {
            for i in 0..36 {
                inventory.set_slot(
                    PlayerSlot::Storage(i),
                    ItemStack::new("minecraft:stone".into(), 64, 64, Tag::Compound(vec![])),
                );
            }
        }
        if boolean("matching") {
            inventory.set_slot(PlayerSlot::Storage(5), Some(compass(12, Some(&target))));
        }
        let hand = if boolean("offhand") {
            PlayerSlot::Offhand
        } else {
            PlayerSlot::Storage(0)
        };
        inventory.set_slot(
            hand,
            Some(compass(
                row["count"].as_u64().unwrap() as u32,
                boolean("old").then_some(&old),
            )),
        );
        let dropped = inventory
            .bind_lodestone_compass(hand, &target, boolean("creative"))
            .unwrap();
        assert!(boolean("success"));
        assert_eq!(row["sounds"], 1);
        assert_eq!(
            describe(dropped.as_ref(), &target),
            row["drop"].as_str().unwrap(),
            "{row}"
        );
        for i in 0..36 {
            assert_eq!(
                describe(inventory.slot(PlayerSlot::Storage(i)), &target),
                row["slots"][usize::from(i)].as_str().unwrap(),
                "slot {i} {row}"
            );
        }
        assert_eq!(
            describe(inventory.slot(PlayerSlot::Offhand), &target),
            row["off"].as_str().unwrap(),
            "{row}"
        );
    }
}
