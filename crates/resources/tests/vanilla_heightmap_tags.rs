//! Compare data-driven heightmap tags with the installed Java 26.3 tag loader.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_resources::{
    datapacks::DataPackSettings, heightmap_tags::HeightmapTags, packs::ResourceStack,
};
use serde_json::Value;

const JAVA: &str = r#"
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.TagLoader;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.DataPackConfig;
import net.minecraft.world.level.WorldDataConfiguration;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.validation.DirectoryValidator;

public class HeightmapTagsOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        JsonArray scenarios = new JsonArray();
        for (int scenario=0; scenario<2; scenario++) {
            var repo = ServerPacksSource.createPackRepository(Path.of(args[0]), new DirectoryValidator(path -> false));
            var selected = scenario == 0 ? List.of("vanilla") : List.of("vanilla", "file/heights");
            var disabled = scenario == 0 ? List.of("file/heights") : List.<String>of();
            var config = new WorldDataConfiguration(new DataPackConfig(selected, disabled), FeatureFlags.DEFAULT_FLAGS);
            MinecraftServer.configurePackRepository(repo, config, false, false);
            try (var manager = new MultiPackResourceManager(PackType.SERVER_DATA, repo.openAllSelected())) {
                TagLoader.ElementLookup<Holder<Block>> lookup = (id, required) -> BuiltInRegistries.BLOCK.get(id).map(holder -> (Holder<Block>)holder);
                var tags = TagLoader.loadTagsForRegistry(manager, Registries.BLOCK, lookup);
                var motion = tags.get(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP);
                var noLeaves = tags.get(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES);
                if (motion == null || noLeaves == null) throw new IllegalStateException("heightmap tags missing");
                JsonArray rows = new JsonArray();
                for (Block block : BuiltInRegistries.BLOCK) {
                    JsonArray row = new JsonArray();
                    row.add(BuiltInRegistries.BLOCK.getKey(block).toString());
                    row.add(motion.contains(block.builtInRegistryHolder()));
                    row.add(noLeaves.contains(block.builtInRegistryHolder()));
                    rows.add(row);
                }
                scenarios.add(rows);
            }
        }
        Files.writeString(Path.of(args[1]), scenarios.toString());
    }
}
"#;

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
        "Java tag oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn heightmap_tags_match_vanilla_with_a_world_data_pack_override() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let world = temp.path().join("world");
    let packs = world.join("datapacks");
    let custom = packs.join("heights");
    let tags_dir = custom.join("data/minecraft/tags/block");
    std::fs::create_dir_all(&tags_dir).unwrap();
    std::fs::write(
        custom.join("pack.mcmeta"),
        br#"{"pack":{"description":"heightmap test","min_format":121,"max_format":121}}"#,
    )
    .unwrap();
    std::fs::write(tags_dir.join("blocks_motion_in_heightmap.json"), br##"{"replace":true,"values":["minecraft:glass","#minecraft:planks",{"id":"minecraft:absent_for_test","required":false}]}"##).unwrap();
    std::fs::write(
        tags_dir.join("blocks_motion_in_heightmap_no_leaves.json"),
        br#"{"replace":true,"values":["minecraft:grass_block"]}"#,
    )
    .unwrap();
    let source = temp.path().join("HeightmapTagsOracle.java");
    let result = temp.path().join("tags.json");
    std::fs::write(&source, JAVA).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("HeightmapTagsOracle")
        .arg(&packs)
        .arg(&result));
    let scenarios: Value = serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
    let registry: BTreeSet<String> = scenarios[0]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[0].as_str().unwrap().to_owned())
        .collect();
    assert!(registry.len() > 1000);
    for mode in 0..2 {
        let mut resources = ResourceStack::open(&game).unwrap();
        if mode == 1 {
            let warnings = resources
                .configure_world_data(
                    &world,
                    &DataPackSettings {
                        enabled: vec!["vanilla".into(), "file/heights".into()],
                        disabled: vec![],
                        enabled_features: vec!["minecraft:vanilla".into()],
                    },
                )
                .unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
        }
        let tags = HeightmapTags::load(&mut resources, &registry).unwrap();
        for row in scenarios[mode].as_array().unwrap() {
            let name = row[0].as_str().unwrap();
            let flags = tags.for_block(name).unwrap();
            assert_eq!(
                flags.blocks_motion,
                row[1].as_bool().unwrap(),
                "mode {mode}, block {name}"
            );
            assert_eq!(
                flags.blocks_motion_no_leaves,
                row[2].as_bool().unwrap(),
                "mode {mode}, block {name}"
            );
        }
    }
    println!(
        "Compared {} registered blocks across vanilla and a data pack",
        registry.len()
    );
}
