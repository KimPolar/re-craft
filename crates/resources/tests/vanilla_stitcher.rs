//! Compare sprite positions with the installed Java 26.3 Stitcher.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_resources::{
    packs::ResourceId,
    vanilla_stitcher::{SpriteSize, stitch},
};
use serde_json::{Value, json};

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).unwrap()
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java stitcher failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn synthetic_sprite_layouts_match_java_26_3() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("StitcherOracle.java");
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/StitcherOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));

    let mut state = 0x9183_7531_u32;
    let mut random = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state
    };
    let mut cases = Vec::new();
    let mut expected = Vec::new();
    for index in 0..320 {
        let mip = (index % 5) as u32;
        let anisotropy = ((index / 5) % 5) as u32;
        let limit = [256, 512, 1024, 2048][index % 4];
        let mut sprites = Vec::new();
        let mut values = Vec::new();
        for item in 0..(1 + index % 48) {
            let id = ResourceId::parse(&format!(
                "{}:sprite_{:03}",
                if item % 3 == 0 { "zeta" } else { "alpha" },
                (item * 17 + index * 3) % 1024
            ))
            .unwrap();
            let width = 1 + random() % 80;
            let height = 1 + random() % 80;
            values.push(
                json!({"id":format!("{}:{}",id.namespace,id.path),"width":width,"height":height}),
            );
            sprites.push(SpriteSize { id, width, height });
        }
        cases.push(json!({"max_width":limit,"max_height":limit,"mip":mip,"anisotropy":anisotropy,"sprites":values}));
        let actual = match stitch(&sprites, limit, limit, mip, anisotropy) {
            Ok(layout) => {
                let placements: serde_json::Map<String, Value> = layout
                    .placements
                    .iter()
                    .map(|(id, p)| {
                        (
                            format!("{}:{}", id.namespace, id.path),
                            json!([p.x, p.y, p.padding]),
                        )
                    })
                    .collect();
                json!({"width":layout.width,"height":layout.height,"placements":placements})
            }
            Err(_) => json!({"error":true}),
        };
        expected.push(actual);
    }
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("StitcherOracle")
        .arg(&input)
        .arg(&output));
    let actual: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(actual.len(), cases.len());
    for (index, (java, rust)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(java, rust, "stitch case {index}: {}", cases[index]);
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn installed_item_atlas_matches_every_java_sprite() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("ItemAtlasOracle.java");
    let output = temp.path().join("atlas.json");
    std::fs::write(&source, include_str!("java/ItemAtlasOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let pack = temp.path().join("pack");
    let make_image = |path: &str, width, height| {
        let path = pack.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbaImage::from_pixel(width, height, image::Rgba([180, 30, 110, 255]))
            .save(path)
            .unwrap();
    };
    make_image("assets/minecraft/textures/item/diamond_sword.png", 32, 32);
    make_image("assets/demo/textures/item/new.png", 16, 16);
    make_image("assets/demo/textures/item/animated.png", 8, 16);
    make_image("assets/demo/textures/bonus/single.png", 20, 12);
    std::fs::write(
        pack.join("assets/demo/textures/item/animated.png.mcmeta"),
        br#"{"animation":{}}"#,
    )
    .unwrap();
    let definition = pack.join("assets/minecraft/atlases/items.json");
    std::fs::create_dir_all(definition.parent().unwrap()).unwrap();
    std::fs::write(definition, serde_json::to_vec(&json!({"sources":[
        {"type":"minecraft:filter","pattern":{"namespace":"minecraft","path":"item/apple"}},
        {"type":"minecraft:directory","source":"bonus","prefix":"extra/"},
        {"type":"minecraft:single","resource":"demo:bonus/single","sprite":"demo:display/custom"}
    ]})).unwrap()).unwrap();
    for with_pack in [false, true] {
        let mut java_run = Command::new(java.join("java.exe"));
        java_run
            .arg("-cp")
            .arg(&cp)
            .arg("ItemAtlasOracle")
            .arg(&output);
        if with_pack {
            java_run.arg(&pack);
        }
        run(&mut java_run);
        let expected: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let mut stack = native_resources::packs::ResourceStack::open(&game).unwrap();
        if with_pack {
            stack.push_pack(&pack).unwrap();
        }
        let started = std::time::Instant::now();
        let atlas = native_resources::vanilla_atlas::item_layout(&mut stack).unwrap();
        eprintln!(
            "item atlas layout (pack {with_pack}): {:?}",
            started.elapsed()
        );
        let actual: serde_json::Map<String, Value> = atlas
            .placements
            .iter()
            .map(|(id, p)| {
                (
                    format!("{}:{}", id.namespace, id.path),
                    json!([p.x, p.y, p.width, p.height]),
                )
            })
            .collect();
        assert_eq!(atlas.width, expected["width"].as_u64().unwrap() as u32);
        assert_eq!(atlas.height, expected["height"].as_u64().unwrap() as u32);
        assert_eq!(
            actual,
            *expected["sprites"].as_object().unwrap(),
            "with pack: {with_pack}"
        );
        if with_pack {
            assert_eq!(actual["minecraft:item/diamond_sword"][2], 32);
            assert_eq!(actual["demo:item/animated"][3], 8);
            assert!(actual.contains_key("demo:display/custom"));
            assert!(!actual.contains_key("minecraft:item/apple"));
        }
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn installed_block_atlas_matches_every_java_sprite_at_multiple_mip_levels() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("ItemAtlasOracle.java");
    let output = temp.path().join("atlas.json");
    std::fs::write(&source, include_str!("java/ItemAtlasOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let pack = temp.path().join("pack");
    let override_image = pack.join("assets/minecraft/textures/block/stone.png");
    std::fs::create_dir_all(override_image.parent().unwrap()).unwrap();
    image::RgbaImage::from_pixel(20, 12, image::Rgba([80, 90, 100, 255]))
        .save(override_image)
        .unwrap();
    for (with_pack, mip) in [(false, 0), (false, 4), (true, 4)] {
        let mut java_run = Command::new(java.join("java.exe"));
        java_run
            .arg("-cp")
            .arg(&cp)
            .arg("ItemAtlasOracle")
            .arg(&output)
            .arg(if with_pack {
                pack.as_os_str()
            } else {
                std::ffi::OsStr::new("-")
            })
            .arg("blocks")
            .arg(mip.to_string());
        run(&mut java_run);
        let expected: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let mut stack = native_resources::packs::ResourceStack::open(&game).unwrap();
        if with_pack {
            stack.push_pack(&pack).unwrap();
        }
        let atlas = native_resources::vanilla_atlas::block_layout(&mut stack, mip).unwrap();
        let actual: serde_json::Map<String, Value> = atlas
            .placements
            .iter()
            .map(|(id, p)| {
                (
                    format!("{}:{}", id.namespace, id.path),
                    json!([p.x, p.y, p.width, p.height]),
                )
            })
            .collect();
        assert_eq!(atlas.width, expected["width"].as_u64().unwrap() as u32);
        assert_eq!(atlas.height, expected["height"].as_u64().unwrap() as u32);
        assert_eq!(
            actual,
            *expected["sprites"].as_object().unwrap(),
            "pack: {with_pack}, requested mip: {mip}"
        );
        if with_pack {
            assert_eq!(actual["minecraft:block/stone"][2], 20);
        }
    }
}
