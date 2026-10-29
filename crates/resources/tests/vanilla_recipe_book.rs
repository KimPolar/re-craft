//! Compare recipe-book fit, layout and ingredient availability with Java 26.3.

use native_resources::{
    packs::{ResourceId, ResourceProvider, ResourceStack},
    recipes::CraftingRecipes,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

const ORACLE: &str = r#"
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.recipebook.PlaceRecipeHelper;
import net.minecraft.world.entity.player.StackedItemContents;
import net.minecraft.world.item.*;
import net.minecraft.world.item.crafting.*;

public class RecipeBookOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    for (var item : List.of(Items.STONE,Items.DIRT))
      item.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
    var stone=Ingredient.of(Items.STONE); var dirt=Ingredient.of(Items.DIRT);
    var either=Ingredient.of(Items.STONE,Items.DIRT);
    var info=new Recipe.CommonInfo(true);
    var book=new CraftingRecipe.CraftingBookInfo(CraftingBookCategory.MISC, "");
    var result=new ItemStackTemplate(Items.STONE);
    List<Recipe<?>> recipes=List.of(
      new ShapedRecipe(info,book,ShapedRecipePattern.of(Map.of('A',stone),"A"),result),
      new ShapedRecipe(info,book,ShapedRecipePattern.of(Map.of('A',stone),"A","A"),result),
      new ShapedRecipe(info,book,ShapedRecipePattern.of(Map.of('A',either,'B',stone),"AB"," A"),result),
      new ShapelessRecipe(info,book,result,List.of(either,stone)),
      new ShapelessRecipe(info,book,result,List.of(either,stone,dirt,either,stone)));
    JsonArray input=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    JsonArray output=new JsonArray();
    for (var entry: input) {
      var value=entry.getAsJsonObject(); var recipe=recipes.get(value.get("recipe").getAsInt());
      int width=value.get("width").getAsInt(), height=value.get("height").getAsInt();
      boolean fits = recipe instanceof ShapedRecipe s ? s.getWidth()<=width && s.getHeight()<=height
        : recipe.placementInfo().ingredients().size()<=width*height;
      JsonArray response=new JsonArray(), layout=new JsonArray();
      boolean[] cells=new boolean[width*height];
      if (fits) PlaceRecipeHelper.placeRecipe(width,height,recipe,recipe.placementInfo().slotsToIngredientIndex(),
        (Integer ingredient,int slot,int x,int y) -> cells[slot]=ingredient>=0);
      for (boolean cell:cells) layout.add(cell);
      var available=new StackedItemContents(); var counts=value.getAsJsonArray("counts");
      if (counts.get(0).getAsInt()>0) available.accountStack(new ItemStack(Items.STONE,counts.get(0).getAsInt()));
      if (counts.get(1).getAsInt()>0) available.accountStack(new ItemStack(Items.DIRT,counts.get(1).getAsInt()));
      response.add(fits); response.add(layout);
      response.add(fits && available.canCraft(recipe,value.get("amount").getAsInt(),null)); output.add(response);
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
                    path.strip_prefix("data/test/recipe/")
                        .unwrap()
                        .strip_suffix(".json")
                        .unwrap(),
                )
                .map(|mut id| {
                    id.namespace = "test".into();
                    id
                })
            })
            .collect()
    }
}

fn java_bin() -> PathBuf {
    std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin")
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
        "Java recipe-book oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn recipe_book_layout_and_craftability_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = ResourceStack::open(&game).unwrap();
    let mut provider = Recipes::default();
    let recipes = [
        json!({"type":"minecraft:crafting_shaped","pattern":["A"],"key":{"A":"minecraft:stone"},"result":{"id":"minecraft:stone"}}),
        json!({"type":"minecraft:crafting_shaped","pattern":["A","A"],"key":{"A":"minecraft:stone"},"result":{"id":"minecraft:stone"}}),
        json!({"type":"minecraft:crafting_shaped","pattern":["AB"," A"],"key":{"A":["minecraft:stone","minecraft:dirt"],"B":"minecraft:stone"},"result":{"id":"minecraft:stone"}}),
        json!({"type":"minecraft:crafting_shapeless","ingredients":[["minecraft:stone","minecraft:dirt"],"minecraft:stone"],"result":{"id":"minecraft:stone"}}),
        json!({"type":"minecraft:crafting_shapeless","ingredients":[["minecraft:stone","minecraft:dirt"],"minecraft:stone","minecraft:dirt",["minecraft:stone","minecraft:dirt"],"minecraft:stone"],"result":{"id":"minecraft:stone"}}),
    ];
    for (index, value) in recipes.iter().enumerate() {
        provider.0.insert(
            format!("data/test/recipe/{index}.json"),
            serde_json::to_vec(value).unwrap(),
        );
    }
    let recipes = CraftingRecipes::load(&mut provider).unwrap();
    assert!(recipes.warnings.is_empty(), "{:?}", recipes.warnings);
    let mut cases = Vec::new();
    for recipe in 0..5 {
        for size in [2, 3] {
            for stone in 0..6 {
                for dirt in 0..6 {
                    for amount in 1..=4 {
                        cases.push(json!({"recipe":recipe,"width":size,"height":size,"counts":[stone,dirt],"amount":amount}));
                    }
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("RecipeBookOracle.java");
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
        .arg("RecipeBookOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    for (case, expected) in cases.iter().zip(expected) {
        let id = ResourceId::parse(&format!("test:{}", case["recipe"].as_u64().unwrap())).unwrap();
        let recipe = recipes.get(&id).unwrap();
        let size = case["width"].as_u64().unwrap() as usize;
        let layout = recipe.book_layout(size, size);
        let occupied: Vec<_> = layout
            .as_ref()
            .map(|slots| slots.iter().map(Option::is_some).collect())
            .unwrap_or_else(|| vec![false; size * size]);
        let available = [
            (
                "minecraft:stone",
                case["counts"][0].as_u64().unwrap() as u32,
            ),
            ("minecraft:dirt", case["counts"][1].as_u64().unwrap() as u32),
        ];
        let can = recipe
            .book_ingredients(
                size,
                size,
                &recipes.tags,
                &available,
                case["amount"].as_u64().unwrap() as u32,
            )
            .is_some();
        assert_eq!(json!([layout.is_some(), occupied, can]), expected, "{case}");
    }
    println!(
        "Compared {} recipe-book layouts and availability cases with Java 26.3",
        cases.len()
    );
}
