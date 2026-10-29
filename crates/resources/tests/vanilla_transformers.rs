//! Differential check of vanilla's data-driven axe transformation providers.

use native_resources::{
    packs::ResourceStack,
    transforms::{AxeTransformers, BlockTransformers, State, ToolKind},
};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

const ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import net.minecraft.world.item.component.BlockTransformers;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.RotatedPillarBlock;
import net.minecraft.world.level.block.state.BlockState;

public class AxeTransformOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var world = VanillaRegistries.createWorldLookup();
    var lookup = VanillaRegistries.createReloadableLookup(world);
    var axe = lookup.lookupOrThrow(Registries.BLOCK_TRANSFORMER).getOrThrow(BlockTransformers.AXE).value();
    JsonArray rows = new JsonArray();
    for(int i=1;i<args.length;i++) {
      String id = args[i];
      var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(id));
      if (block == null) throw new IllegalStateException("missing block " + id);
      for(var axis : Direction.Axis.values()) {
        BlockState original = block.defaultBlockState().setValue(RotatedPillarBlock.AXIS,axis);
        LevelAccessor level = (LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),
          new Class<?>[]{LevelAccessor.class}, (proxy,method,methodArgs) -> {
            if (method.getName().equals("getBlockState")) return original;
            throw new IllegalStateException("unexpected world call " + method.getName());
          });
        for(var face : Direction.values()) {
          BlockState result = null; int damage = 0; boolean update = false;
          for(var entry : axe.transforms()) {
            if(entry.disallowedFaces().contains(face)) continue;
            result = entry.blockStateProvider().value().getOptionalState(level, RandomSource.create(1L), BlockPos.ZERO);
            if(result != null) { damage=entry.itemDamagePerUse(); update=entry.updateFromNeighbors(); break; }
          }
          JsonArray row = new JsonArray(); row.add(id); row.add(axis.getName()); row.add(face.getName());
          if(result == null) { row.add(JsonNull.INSTANCE); }
          else {
            row.add(BuiltInRegistries.BLOCK.getKey(result.getBlock()).toString());
            row.add(result.getValue(RotatedPillarBlock.AXIS).getName());
            row.add(damage); row.add(update);
          }
          rows.add(row);
        }
      }
    }
    Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
  }
}
"#;

const SOIL_ORACLE: &str = r##"
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import java.util.zip.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Holder;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.TagKey;
import net.minecraft.tags.TagLoader;
import net.minecraft.util.RandomSource;
import net.minecraft.world.item.component.BlockTransformers;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;

public class SoilTransformOracle {
  static JsonObject properties(BlockState state) {
    JsonObject result=new JsonObject();
    state.getValues().forEach(value -> result.addProperty(value.property().getName(),value.valueName()));
    return result;
  }
  static BlockState state(String id) {
    var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse(id));
    if(block==null) throw new IllegalStateException("unknown block "+id);
    return block.defaultBlockState();
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    Map<TagKey<Block>,List<Holder<Block>>> tags=new HashMap<>();
    try (var jar=new ZipFile(args[1])) {
      for(String name : List.of("turns_into_dirt_path", "turns_into_farmland", "air")) {
        var entry=jar.getEntry("data/minecraft/tags/block/"+name+".json");
        var json=JsonParser.parseString(new String(jar.getInputStream(entry).readAllBytes(),java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject();
        List<Holder<Block>> values=new ArrayList<>();
        for(var element:json.getAsJsonArray("values")) {
          String id=element.getAsString();
          if(id.startsWith("#")) throw new IllegalStateException("unexpected nested vanilla tag");
          values.add(BuiltInRegistries.BLOCK.getValue(Identifier.parse(id)).builtInRegistryHolder());
        }
        tags.put(TagKey.create(Registries.BLOCK,Identifier.withDefaultNamespace(name)),values);
      }
    }
    ((MappedRegistry<Block>)BuiltInRegistries.BLOCK).prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,tags)).apply();
    var lookup=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    JsonArray rows=new JsonArray();
    for(String tool:List.of("shovel","hoe")) {
      var key=tool.equals("hoe") ? BlockTransformers.HOE : BlockTransformers.SHOVEL;
      var transformer=lookup.lookupOrThrow(Registries.BLOCK_TRANSFORMER).getOrThrow(key).value();
      for(String input:List.of("grass_block","dirt","coarse_dirt","rooted_dirt","podzol","mycelium","dirt_path","farmland","stone"))
      for(String upper:List.of("air","cave_air","void_air","stone","water")) {
        var original=state("minecraft:"+input); var above=state("minecraft:"+upper);
        LevelAccessor level=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class<?>[]{LevelAccessor.class},(p,m,a)-> {
          if(m.getName().equals("getBlockState")) {
            BlockPos pos=(BlockPos)a[0];
            if(pos.equals(BlockPos.ZERO)) return original;
            if(pos.equals(BlockPos.ZERO.above())) return above;
          }
          throw new IllegalStateException("unexpected world call "+m.getName());
        });
        for(var face:Direction.values()) {
          BlockState result=null; int damage=0; boolean update=false, loot=false; String lootId="", strategy="";
          for(var entry:transformer.transforms()) {
            if(entry.disallowedFaces().contains(face)) continue;
            result=entry.blockStateProvider().value().getOptionalState(level,RandomSource.create(1L),BlockPos.ZERO);
            if(result!=null) { damage=entry.itemDamagePerUse(); update=entry.updateFromNeighbors(); loot=entry.loot().isPresent(); if(loot) { lootId=entry.loot().get().identifier().toString(); strategy=entry.dropStrategy().getSerializedName(); } break; }
          }
          JsonArray row=new JsonArray(); row.add(tool); row.add("minecraft:"+input); row.add(properties(original));
          row.add("minecraft:"+upper); row.add(face.getName());
          if(result==null) row.add(JsonNull.INSTANCE);
          else { row.add(BuiltInRegistries.BLOCK.getKey(result.getBlock()).toString()); row.add(properties(result)); row.add(damage); row.add(update); row.add(loot); if(loot) { row.add(lootId); row.add(strategy); } }
          rows.add(row);
        }
      }
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
  }
}
"##;

fn java_bin() -> PathBuf {
    std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
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
        "axe oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn axe_transformers_match_the_local_vanilla_registry() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut source = ResourceStack::open(&game).unwrap();
    let native = AxeTransformers::load(&mut source).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let java_file = temp.path().join("AxeTransformOracle.java");
    let output = temp.path().join("output.json");
    std::fs::write(&java_file, ORACLE).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&java_file));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("AxeTransformOracle")
        .arg(&output)
        .args(native_core::wood::Wood::all().map(|wood| wood.id())));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), native_core::wood::Wood::all().count() * 3 * 6);
    for row in &rows {
        let state = State {
            name: row[0].as_str().unwrap().into(),
            properties: std::collections::BTreeMap::from([(
                "axis".into(),
                row[1].as_str().unwrap().into(),
            )]),
        };
        let result = native.apply(&state, row[2].as_str().unwrap()).unwrap();
        if row[3].is_null() {
            assert!(result.is_none(), "{row}");
        } else {
            let result = result.unwrap();
            assert_eq!(result.state.name, row[3].as_str().unwrap(), "{row}");
            assert_eq!(
                result.state.properties.get("axis").map(String::as_str),
                row[4].as_str(),
                "{row}"
            );
            assert_eq!(u64::from(result.damage), row[5].as_u64().unwrap(), "{row}");
            assert_eq!(
                result.update_from_neighbors,
                row[6].as_bool().unwrap(),
                "{row}"
            );
        }
    }
    eprintln!(
        "Compared {} wood, axis, and face transformations with Java 26.3",
        rows.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn shovel_and_hoe_transformers_match_vanilla_soil_rules() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut source = ResourceStack::open(&game).unwrap();
    let shovel = BlockTransformers::load_for(&mut source, ToolKind::Shovel).unwrap();
    let hoe = BlockTransformers::load_for(&mut source, ToolKind::Hoe).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let java_file = temp.path().join("SoilTransformOracle.java");
    let output = temp.path().join("soil.json");
    std::fs::write(&java_file, SOIL_ORACLE).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&java_file));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SoilTransformOracle")
        .arg(&output)
        .arg(game.join("versions/26.3/26.3.jar")));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 2 * 9 * 5 * 6);
    let mut interaction_loot = 0;
    for row in &rows {
        let properties = |value: &Value| {
            value
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
                .collect()
        };
        let state = State {
            name: row[1].as_str().unwrap().into(),
            properties: properties(&row[2]),
        };
        let above = State {
            name: row[3].as_str().unwrap().into(),
            properties: Default::default(),
        };
        let transforms = if row[0] == "hoe" { &hoe } else { &shovel };
        let result = transforms.apply_at(&state, row[4].as_str().unwrap(), |offset| {
            assert_eq!(offset, [0, 1, 0]);
            Some(above.clone())
        });
        if row[5].is_null() {
            assert!(result.unwrap().is_none(), "{row}");
        } else {
            let result = result.unwrap().unwrap();
            assert_eq!(result.state.name, row[5].as_str().unwrap(), "{row}");
            assert_eq!(result.state.properties, properties(&row[6]), "{row}");
            assert_eq!(u64::from(result.damage), row[7].as_u64().unwrap(), "{row}");
            assert_eq!(
                result.update_from_neighbors,
                row[8].as_bool().unwrap(),
                "{row}"
            );
            if row[9].as_bool() == Some(true) {
                let (id, strategy) = result.loot.unwrap();
                assert_eq!(
                    format!("{}:{}", id.namespace, id.path),
                    row[10].as_str().unwrap(),
                    "{row}"
                );
                assert_eq!(
                    strategy,
                    match row[11].as_str().unwrap() {
                        "clicked_face" => native_resources::transforms::DropStrategy::ClickedFace,
                        "from_middle" => native_resources::transforms::DropStrategy::FromMiddle,
                        other => panic!("unexpected drop strategy {other}"),
                    },
                    "{row}"
                );
                interaction_loot += 1;
            } else {
                assert!(result.loot.is_none(), "{row}");
            }
        }
    }
    eprintln!(
        "Compared {} shovel and hoe rule cases with Java 26.3, including {interaction_loot} interaction-loot cases",
        rows.len()
    );
}
