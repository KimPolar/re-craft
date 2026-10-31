//! Compare builtin biome tint math with the user's installed 26.3 Java client.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_resources::{
    biomes::{BiomeColors, ColorMaps},
    default_minecraft_dir,
    packs::ResourceStack,
};
use serde_json::Value;

const ORACLE: &str = r##"
import java.io.*;
import javax.imageio.ImageIO;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;

public class BiomeColorOracle {
  static int[] map(String name) throws Exception {
    var in = ClassLoader.getSystemResourceAsStream("assets/minecraft/textures/colormap/" + name + ".png");
    var image = ImageIO.read(in);
    return image.getRGB(0, 0, image.getWidth(), image.getHeight(), null, 0, image.getWidth());
  }
  static int rgb(JsonElement value) {
    var s = value.getAsString();
    return s.startsWith("#") ? (int)Long.parseLong(s.substring(1), 16) : value.getAsInt();
  }
  public static void main(String[] args) throws Exception {
    PrintStream output = System.out;
    SharedConstants.tryDetectVersion();
    Bootstrap.bootStrap();
    GrassColor.init(map("grass"));
    FoliageColor.init(map("foliage"));
    DryFoliageColor.init(map("dry_foliage"));
    for (String name : args) {
      var input = ClassLoader.getSystemResourceAsStream("data/minecraft/worldgen/biome/" + name + ".json");
      var json = JsonParser.parseReader(new InputStreamReader(input, java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject();
      var effectsJson = json.getAsJsonObject("effects");
      var effects = new BiomeSpecialEffects.Builder().waterColor(rgb(effectsJson.get("water_color")));
      if (effectsJson.has("grass_color")) effects.grassColorOverride(rgb(effectsJson.get("grass_color")));
      if (effectsJson.has("foliage_color")) effects.foliageColorOverride(rgb(effectsJson.get("foliage_color")));
      if (effectsJson.has("dry_foliage_color")) effects.dryFoliageColorOverride(rgb(effectsJson.get("dry_foliage_color")));
      if (effectsJson.has("grass_color_modifier")) {
        switch (effectsJson.get("grass_color_modifier").getAsString()) {
          case "none" -> { }
          case "dark_forest" -> effects.grassColorModifier(BiomeSpecialEffects.GrassColorModifier.DARK_FOREST);
          case "swamp" -> effects.grassColorModifier(BiomeSpecialEffects.GrassColorModifier.SWAMP);
          default -> throw new IllegalArgumentException("unsupported grass modifier");
        }
      }
      var biome = new Biome.BiomeBuilder().temperature(json.get("temperature").getAsFloat())
        .downfall(json.get("downfall").getAsFloat()).specialEffects(effects.build())
        .generationSettings(BiomeGenerationSettings.EMPTY).build();
      output.printf("%s %d %d %d %d%n", name, biome.getGrassColor(12, -8) & 0xffffff,
        biome.getFoliageColor() & 0xffffff, biome.getDryFoliageColor() & 0xffffff,
        biome.getWaterColor() & 0xffffff);
    }
  }
}
"##;

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
        "Java biome color oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn builtin_biome_colors_match_vanilla_including_swamp() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_minecraft_dir().unwrap());
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("BiomeColorOracle.java");
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
    let jar = std::fs::File::open(game.join("versions/26.3/26.3.jar")).unwrap();
    let archive = zip::ZipArchive::new(jar).unwrap();
    let names: Vec<String> = archive
        .file_names()
        .filter_map(|name| {
            name.strip_prefix("data/minecraft/worldgen/biome/")?
                .strip_suffix(".json")
        })
        .filter(|name| !name.contains('/'))
        .map(str::to_owned)
        .collect();
    assert!(names.len() >= 60, "expected builtin biome definitions");
    let result = run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-Djava.awt.headless=true")
        .arg("-classpath")
        .arg(&cp)
        .arg("BiomeColorOracle")
        .args(&names));
    let mut resources = ResourceStack::open(&game).unwrap();
    let mut warnings = Vec::new();
    let maps = ColorMaps::load(&mut resources, &mut warnings);
    assert!(warnings.is_empty(), "{warnings:?}");
    let mut count = 0;
    for line in result.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 5, "unexpected oracle output {line}");
        let biome = BiomeColors::load_builtin(&mut resources, parts[0], &maps)
            .unwrap()
            .unwrap();
        let values: Vec<u32> = parts[1..]
            .iter()
            .map(|value| value.parse().unwrap())
            .collect();
        assert_eq!(biome.grass_at(12, -8), values[0], "{} grass", parts[0]);
        assert_eq!(biome.foliage, values[1], "{} foliage", parts[0]);
        assert_eq!(biome.dry_foliage, values[2], "{} dry foliage", parts[0]);
        assert_eq!(biome.water, values[3], "{} water", parts[0]);
        count += 1;
    }
    assert_eq!(count, names.len());
}
