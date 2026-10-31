//! Compare world pack discovery, priority, overlays and filters with local 26.3.

use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use native_resources::{
    biomes::{BiomeColors, ColorMaps},
    datapacks::DataPackSettings,
    default_minecraft_dir,
    packs::{ResourceProvider, ResourceStack},
};
use serde_json::Value;

const ORACLE: &str = r#"
import java.io.PrintStream;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.resources.Identifier;
import net.minecraft.world.flag.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.validation.DirectoryValidator;
import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;

public class DataPackOracle {
  public static void main(String[] args) throws Exception {
    PrintStream output = System.out;
    SharedConstants.tryDetectVersion();
    Bootstrap.bootStrap();
    var packdir = Path.of(args[0]);
    List<List<String>> enabled = List.of(
      List.of("vanilla", "file/alpha"),
      List.of("file/beta.zip", "file/alpha", "vanilla"),
      List.of("vanilla", "file/requires"),
      List.of("vanilla", "file/alpha", "file/disabled"),
      List.of("vanilla")
    );
    List<List<String>> disabled = List.of(
      List.of("file/disabled"),
      List.of("file/auto", "file/disabled", "file/requires"),
      List.of("file/disabled"),
      List.of("file/disabled", "file/auto"),
      List.of("file/requires", "file/disabled", "file/auto", "file/alpha", "file/beta.zip")
    );
    String[] paths = {"shared", "overlaid", "hidden", "cross", "same", "legacy", "automatic", "required", "disabled"};
    for (int i = 0; i < enabled.size(); i++) {
      var repo = ServerPacksSource.createPackRepository(packdir, new DirectoryValidator(path -> false));
      var features = (i == 2 || i == 4) ? FeatureFlags.DEFAULT_FLAGS.join(FeatureFlagSet.of(FeatureFlags.TRADE_REBALANCE)) : FeatureFlags.DEFAULT_FLAGS;
      var config = new WorldDataConfiguration(new DataPackConfig(enabled.get(i), disabled.get(i)), features);
      MinecraftServer.configurePackRepository(repo, config, false, false);
      output.println("S " + i + " " + String.join(",", repo.getSelectedPacks().stream().map(p -> p.getId()).toList()));
      try (var manager = new MultiPackResourceManager(PackType.SERVER_DATA, repo.openAllSelected())) {
        for (String name : paths) {
          var resource = manager.getResource(Identifier.parse("example:worldgen/biome/" + name + ".json"));
          output.println("R " + i + " " + name + " " + (resource.isPresent() ? resource.get().readAllAsString() : "-"));
          List<String> versions = new ArrayList<>();
          for (var layer : manager.getResourceStack(Identifier.parse("example:worldgen/biome/" + name + ".json"))) versions.add(layer.readAllAsString());
          output.println("T " + i + " " + name + " " + (versions.isEmpty() ? "-" : String.join("|", versions)));
        }
        var names = manager.listResources("worldgen/biome", id -> id.getPath().endsWith(".json")).keySet().stream()
          .filter(id -> id.getNamespace().equals("example"))
          .map(id -> id.getPath().substring("worldgen/biome/".length(), id.getPath().length()-5)).sorted().toList();
        output.println("L " + i + " " + (names.isEmpty() ? "-" : String.join(",", names)));
        if (i == 0) {
          var resource = manager.getResource(Identifier.parse("example:worldgen/biome/custom.json")).orElseThrow();
          var biome = Biome.DIRECT_CODEC.parse(JsonOps.INSTANCE, JsonParser.parseString(resource.readAllAsString())).getOrThrow();
          output.println("C " + (biome.getWaterColor() & 0xffffff));
        }
      }
    }
  }
}
"#;

fn java_bin() -> PathBuf {
    if let Some(path) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(path).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").expect("set MINECRAFT_JAVA_HOME to Java 25 JDK"))
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
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

fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "Java data pack oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn write_file(root: &Path, path: &str, bytes: &[u8]) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn folder_pack(root: &Path, name: &str, features: &str, files: &[(&str, &str)]) {
    let root = root.join(name);
    write_file(
        &root,
        "pack.mcmeta",
        format!(
            r#"{{"pack":{{"description":"test","min_format":121,"max_format":121}}{features}}}"#
        )
        .as_bytes(),
    );
    for (path, value) in files {
        write_file(
            &root,
            &format!("data/example/worldgen/biome/{path}.json"),
            value.as_bytes(),
        );
    }
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn pack_selection_and_resources_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_minecraft_dir().unwrap());
    let temp = tempfile::tempdir().unwrap();
    let world = temp.path().join("world");
    let packs = world.join("datapacks");
    std::fs::create_dir_all(&packs).unwrap();
    folder_pack(
        &packs,
        "alpha",
        "",
        &[
            ("shared", "alpha"),
            ("hidden", "hidden_alpha"),
            ("cross", "cross_alpha"),
            ("overlaid", "alpha_overlay"),
            ("nested/only", "nested"),
        ],
    );
    folder_pack(&packs, "auto", "", &[("automatic", "auto")]);
    write_file(
        &packs.join("alpha"),
        "data/example/worldgen/biome/custom.json",
        br##"{"has_precipitation":true,"temperature":0.8,"downfall":0.4,"effects":{"water_color":"#123456","grass_color":"#abcdef"},"carvers":[],"features":[]}"##,
    );
    write_file(
        &packs.join("alpha"),
        "data/minecraft/tags/fluid/water.json",
        br##"{"replace":true,"values":["#example:hot"]}"##,
    );
    write_file(
        &packs.join("alpha"),
        "data/example/tags/fluid/hot.json",
        br#"{"values":["minecraft:lava"]}"#,
    );
    folder_pack(&packs, "disabled", "", &[("disabled", "disabled")]);
    folder_pack(
        &packs,
        "requires",
        r#", "features":{"enabled":["minecraft:trade_rebalance"]}"#,
        &[("required", "requires")],
    );
    let mut zip = zip::ZipWriter::new(std::fs::File::create(packs.join("beta.zip")).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    let metadata = br#"{"pack":{"description":"test","min_format":121,"max_format":121},"overlays":{"entries":[{"directory":"layer_one","min_format":121,"max_format":121},{"directory":"layer_two","min_format":[121,0],"max_format":[121,0]},{"directory":"future","min_format":[121,1],"max_format":[121,9]}]},"filter":{"block":[{"namespace":"example","path":"worldgen/biome/hidden\\.json"},{"namespace":"other","path":"worldgen/biome/cross\\.json"},{"namespace":"example","path":"worldgen/biome/same\\.json"}]}}"#;
    for (path, value) in [
        ("pack.mcmeta", metadata.as_slice()),
        (
            "data/example/worldgen/biome/shared.json",
            b"beta".as_slice(),
        ),
        (
            "data/example/worldgen/biome/overlaid.json",
            b"base".as_slice(),
        ),
        ("data/example/worldgen/biome/same.json", b"own".as_slice()),
        (
            "layer_one/data/example/worldgen/biome/overlaid.json",
            b"one".as_slice(),
        ),
        (
            "layer_two/data/example/worldgen/biome/overlaid.json",
            b"two".as_slice(),
        ),
        (
            "layer_two/data/example/worldgen/biome/overlay_only.json",
            b"extra".as_slice(),
        ),
        (
            "future/data/example/worldgen/biome/overlaid.json",
            b"future".as_slice(),
        ),
    ] {
        zip.start_file(path, options).unwrap();
        zip.write_all(value).unwrap();
    }
    zip.finish().unwrap();
    let mut legacy = zip::ZipWriter::new(std::fs::File::create(packs.join("legacy.zip")).unwrap());
    for (path, value) in [
        ("pack.mcmeta", br#"{"pack":{"description":"legacy","min_format":121,"max_format":121},"overlays":{"entries":[{"directory":"old","formats":[80,81]},{"directory":"current","min_format":121,"max_format":121,"formats":[121,121]}]}}"#.as_slice()),
        ("data/example/worldgen/biome/legacy.json", b"legacy_base".as_slice()),
        ("old/data/example/worldgen/biome/legacy.json", b"legacy_old".as_slice()),
        ("current/data/example/worldgen/biome/legacy.json", b"legacy_current".as_slice()),
    ] {
        legacy.start_file(path, options).unwrap();
        legacy.write_all(value).unwrap();
    }
    legacy.finish().unwrap();

    let source = temp.path().join("DataPackOracle.java");
    std::fs::write(&source, ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    let java = java_bin();
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let result = run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("DataPackOracle")
        .arg(&packs));
    let make = |enabled: &[&str], disabled: &[&str], trade: bool| DataPackSettings {
        enabled: enabled.iter().map(|s| (*s).to_owned()).collect(),
        disabled: disabled.iter().map(|s| (*s).to_owned()).collect(),
        enabled_features: if trade {
            vec![
                "minecraft:vanilla".into(),
                "minecraft:trade_rebalance".into(),
            ]
        } else {
            vec!["minecraft:vanilla".into()]
        },
    };
    let settings = [
        make(&["vanilla", "file/alpha"], &["file/disabled"], false),
        make(
            &["file/beta.zip", "file/alpha", "vanilla"],
            &["file/auto", "file/disabled", "file/requires"],
            false,
        ),
        make(&["vanilla", "file/requires"], &["file/disabled"], true),
        make(
            &["vanilla", "file/alpha", "file/disabled"],
            &["file/disabled", "file/auto"],
            false,
        ),
        make(
            &["vanilla"],
            &[
                "file/requires",
                "file/disabled",
                "file/auto",
                "file/alpha",
                "file/beta.zip",
            ],
            true,
        ),
    ];
    let mut expected = Vec::new();
    for (index, settings) in settings.iter().enumerate() {
        let mut stack = ResourceStack::open(&game).unwrap();
        let warnings = stack.configure_world_data(&world, settings).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        if index == 0 {
            let biome =
                BiomeColors::load_world(&mut stack, "example:custom", &ColorMaps::default())
                    .unwrap()
                    .unwrap();
            assert_eq!(biome.water, 0x123456);
            assert_eq!(biome.grass_at(0, 0), 0xabcdef);
            assert_eq!(
                native_resources::fluid_tags::load_water(&mut stack).unwrap(),
                std::collections::BTreeSet::from(["minecraft:lava".to_owned()])
            );
        }
        expected.push(format!(
            "S {index} {}",
            stack.selected_data_pack_ids().unwrap().join(",")
        ));
        for name in [
            "shared",
            "overlaid",
            "hidden",
            "cross",
            "same",
            "legacy",
            "automatic",
            "required",
            "disabled",
        ] {
            let path = format!("data/example/worldgen/biome/{name}.json");
            let value = stack
                .read_data(&path)
                .unwrap()
                .map(|bytes| String::from_utf8(bytes).unwrap())
                .unwrap_or_else(|| "-".into());
            expected.push(format!("R {index} {name} {value}"));
            let versions = stack
                .read_data_stack(&path)
                .unwrap()
                .into_iter()
                .map(|bytes| String::from_utf8(bytes).unwrap())
                .collect::<Vec<_>>();
            expected.push(format!(
                "T {index} {name} {}",
                if versions.is_empty() {
                    "-".to_owned()
                } else {
                    versions.join("|")
                }
            ));
        }
        let names: Vec<_> = stack
            .list_data("worldgen/biome")
            .unwrap()
            .into_iter()
            .filter(|id| id.namespace == "example")
            .map(|id| id.path)
            .collect();
        expected.push(format!(
            "L {index} {}",
            if names.is_empty() {
                "-".to_owned()
            } else {
                names.join(",")
            }
        ));
        if index == 0 {
            expected.push(format!("C {}", 0x123456));
        }
    }
    assert_eq!(
        result
            .lines()
            .filter(|line| line.starts_with("S ")
                || line.starts_with("R ")
                || line.starts_with("T ")
                || line.starts_with("L ")
                || line.starts_with("C "))
            .collect::<Vec<_>>(),
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
}
