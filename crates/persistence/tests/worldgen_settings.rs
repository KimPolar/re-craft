use anyhow::Result;
use native_persistence::{
    nbt,
    worldgen_settings::{NewWorldOptions, draft_baked_worldgen_settings, draft_worldgen_settings},
};
use native_resources::packs::{ResourceId, ResourceProvider, ResourceStack};
use std::{path::PathBuf, process::Command};

struct CustomPreset(Vec<u8>);
impl ResourceProvider for CustomPreset {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok((path == "data/example/worldgen/world_preset/custom.json").then(|| self.0.clone()))
    }
}

struct BakedPreset<'a> {
    base: &'a mut ResourceStack,
    overworld: Vec<u8>,
    sky: Vec<u8>,
}
impl ResourceProvider for BakedPreset<'_> {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        match path {
            "data/minecraft/dimension/overworld.json" => Ok(Some(self.overworld.clone())),
            "data/example/dimension/sky.json" => Ok(Some(self.sky.clone())),
            _ => self.base.read_data(path),
        }
    }
    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        assert_eq!(folder, "dimension");
        Ok(vec![
            ResourceId::parse("minecraft:overworld")?,
            ResourceId::parse("example:sky")?,
        ])
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn preset_documents_match_java_worldgen_codec() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let mut stack = ResourceStack::open(&game).unwrap();
    let bytes = stack
        .read_data("data/minecraft/worldgen/world_preset/normal.json")
        .unwrap()
        .unwrap();
    let mut custom: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let end = custom["dimensions"]["minecraft:the_end"].clone();
    let nether = custom["dimensions"]["minecraft:the_nether"].clone();
    {
        let mut baked = BakedPreset {
            base: &mut stack,
            overworld: serde_json::to_vec(&nether).unwrap(),
            sky: serde_json::to_vec(&end).unwrap(),
        };
        let document = draft_baked_worldgen_settings(
            &mut baked,
            "minecraft:normal",
            NewWorldOptions {
                seed: -42,
                generate_structures: false,
                bonus_chest: true,
            },
        )
        .unwrap();
        std::fs::write(
            temp.path().join("baked.nbt"),
            nbt::write(&document).unwrap(),
        )
        .unwrap();
    }
    custom["dimensions"]
        .as_object_mut()
        .unwrap()
        .insert("example:sky".into(), end);
    let mut custom = CustomPreset(serde_json::to_vec(&custom).unwrap());
    let names = [
        "normal",
        "large_biomes",
        "amplified",
        "flat",
        "flat_all_dimensions",
        "single_biome_surface",
        "debug_all_block_states",
        "custom",
    ];
    let seeds = [i64::MIN, -42, 0, i64::MAX];
    for name in names {
        for (index, seed) in seeds.into_iter().enumerate() {
            for flags in 0..4 {
                let options = NewWorldOptions {
                    seed,
                    generate_structures: flags & 1 != 0,
                    bonus_chest: flags & 2 != 0,
                };
                let document = if name == "custom" {
                    draft_worldgen_settings(&mut custom, "example:custom", options).unwrap()
                } else {
                    draft_worldgen_settings(&mut stack, &format!("minecraft:{name}"), options)
                        .unwrap()
                };
                std::fs::write(
                    temp.path().join(format!("{name}-{index}-{flags}.nbt")),
                    nbt::write(&document).unwrap(),
                )
                .unwrap();
            }
        }
    }
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
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
    let source = temp.path().join("WorldGenSettingsOracle.java");
    std::fs::write(&source, include_str!("java/WorldGenSettingsOracle.java")).unwrap();
    let compile = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(source)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("WorldGenSettingsOracle")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
