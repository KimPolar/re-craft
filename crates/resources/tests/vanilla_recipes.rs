//! Compare ordinary shaped and shapeless recipe matching with Java 26.3.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_resources::{
    packs::{ResourceId, ResourceProvider, ResourceStack},
    recipes::CraftingRecipes,
};
use serde_json::{Value, json};

const ORACLE: &str = r#"
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.ItemStackTemplate;
import net.minecraft.world.item.crafting.*;

public class RecipeOracle {
  static ItemStack item(int id) {
    return switch(id) { case 1 -> new ItemStack(Items.STONE); case 2 -> new ItemStack(Items.DIRT); case 3 -> new ItemStack(Items.GRASS_BLOCK); default -> ItemStack.EMPTY; };
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    for (var item : List.of(Items.STONE,Items.DIRT,Items.GRASS_BLOCK))
      item.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
    var stone = Ingredient.of(Items.STONE); var dirt = Ingredient.of(Items.DIRT);
    var either = Ingredient.of(Items.STONE,Items.DIRT);
    var shaped = ShapedRecipePattern.of(Map.of('A',stone,'B',dirt), " AB", " A ");
    var square = ShapedRecipePattern.of(Map.of('S',stone), "SS", "SS");
    var shapeless = new ShapelessRecipe(new Recipe.CommonInfo(true),
      new CraftingRecipe.CraftingBookInfo(CraftingBookCategory.MISC,""),
      new ItemStackTemplate(Items.STONE), List.of(either,stone));
    JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    JsonArray output = new JsonArray();
    for (var test : cases) {
      var obj=test.getAsJsonObject(); int width=obj.get("width").getAsInt(), height=obj.get("height").getAsInt();
      List<ItemStack> slots=new ArrayList<>(); for (var entry:obj.getAsJsonArray("slots")) slots.add(item(entry.getAsInt()));
      var input=CraftingInput.of(width,height,slots);
      JsonArray answers=new JsonArray(); answers.add(shaped.matches(input)); answers.add(square.matches(input)); answers.add(shapeless.matches(input,null)); output.add(answers);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(output));
  }
}
"#;

#[derive(Default)]
struct Recipes(BTreeMap<String, Vec<u8>>);

impl ResourceProvider for Recipes {
    fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self.0.get(path).cloned())
    }
    fn list_data(&mut self, folder: &str) -> anyhow::Result<Vec<ResourceId>> {
        if folder != "recipe" {
            return Ok(vec![]);
        }
        self.0
            .keys()
            .map(|path| {
                ResourceId::parse(
                    path.strip_prefix("data/example/recipe/")
                        .unwrap()
                        .strip_suffix(".json")
                        .unwrap(),
                )
                .map(|mut id| {
                    id.namespace = "example".into();
                    id
                })
            })
            .collect()
    }
}

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
        "Java recipe oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ordinary_recipe_matches_agree_with_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = ResourceStack::open(&game).unwrap();
    let mut provider = Recipes::default();
    for (name, data) in [
        (
            "shaped",
            json!({"type":"minecraft:crafting_shaped","pattern":[" AB"," A "],"key":{"A":"minecraft:stone","B":"minecraft:dirt"},"result":{"id":"minecraft:stone"}}),
        ),
        (
            "square",
            json!({"type":"minecraft:crafting_shaped","pattern":["SS","SS"],"key":{"S":"minecraft:stone"},"result":{"id":"minecraft:stone"}}),
        ),
        (
            "shapeless",
            json!({"type":"minecraft:crafting_shapeless","ingredients":[["minecraft:stone","minecraft:dirt"],"minecraft:stone"],"result":{"id":"minecraft:stone"}}),
        ),
    ] {
        provider.0.insert(
            format!("data/example/recipe/{name}.json"),
            serde_json::to_vec(&data).unwrap(),
        );
    }
    let recipes = CraftingRecipes::load(&mut provider).unwrap();
    assert!(recipes.warnings.is_empty());
    let ids: Vec<_> = ["shaped", "square", "shapeless"]
        .iter()
        .map(|name| ResourceId::parse(&format!("example:{name}")).unwrap())
        .collect();
    let mut cases = Vec::new();
    for encoded in 0..256_u32 {
        let slots: Vec<_> = (0..4).map(|i| (encoded >> (i * 2)) & 3).collect();
        cases.push(json!({"width":2,"height":2,"slots":slots}));
    }
    for encoded in 0..16_384_u32 {
        let random = encoded.wrapping_mul(2654435761);
        let slots: Vec<_> = (0..9).map(|i| (random >> (i * 2)) & 3).collect();
        cases.push(json!({"width":3,"height":3,"slots":slots}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("RecipeOracle.java");
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("RecipeOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    let mut differences = Vec::new();
    for (case, expected) in cases.iter().zip(expected.iter()) {
        let slots: Vec<_> = case["slots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| match value.as_u64().unwrap() {
                1 => Some("minecraft:stone"),
                2 => Some("minecraft:dirt"),
                3 => Some("minecraft:grass_block"),
                _ => None,
            })
            .collect();
        let width = case["width"].as_u64().unwrap() as usize;
        let height = case["height"].as_u64().unwrap() as usize;
        let actual: Vec<_> = ids
            .iter()
            .map(|id| {
                recipes
                    .get(id)
                    .unwrap()
                    .matches(width, height, &slots, &recipes.tags)
            })
            .collect();
        if json!(actual) != *expected {
            differences.push(format!("{case}: got {actual:?}, expected {expected}"));
        }
    }
    assert!(
        differences.is_empty(),
        "{} differences:\n{}",
        differences.len(),
        differences
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!(
        "Compared {} crafting grids against three Java recipes",
        cases.len()
    );
}
