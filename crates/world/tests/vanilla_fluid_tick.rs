//! Compare scheduled fluid changes in controlled air/full-block terrain.

use glam::IVec3;
use native_world::{
    fluid::{FluidKind, FluidState},
    fluid_sim::{self, Cell, ChangedBlock, LavaNeighbor, Settings, Solidified},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

const ORACLE: &str = r#"
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.attribute.*;
import net.minecraft.world.level.gamerules.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.material.*;

public class FluidTickOracle {
  static class TestLevel extends ServerLevel {
    Map<BlockPos,BlockState> blocks; JsonArray changes; GameRules rules;
    EnvironmentAttributeSystem attrs; RandomSource random; boolean lava;
    TestLevel() { super(null,null,null,null,null,null,false,0L,null,false); }
    @Override public BlockState getBlockState(BlockPos p) { return blocks.getOrDefault(p,Blocks.AIR.defaultBlockState()); }
    @Override public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
    @Override public GameRules getGameRules() { return rules; }
    @Override public EnvironmentAttributeSystem environmentAttributes() { return attrs; }
    @Override public RandomSource getRandom() { return random; }
    @Override public void levelEvent(Entity e, int id, BlockPos p, int data) { }
    @Override public void scheduleTick(BlockPos p, Fluid f, int delay) { }
    @Override public boolean setBlockAndUpdate(BlockPos p,BlockState s) {
      BlockPos key=p.immutable(); blocks.put(key,s); var a=new JsonArray(); a.add(p.getX()); a.add(p.getY()); a.add(p.getZ());
      if (s.isAir()) a.add(-1);
      else if (s.is(Blocks.STONE)) a.add(-2);
      else if (s.is(Blocks.OBSIDIAN)) a.add(-3);
      else if (s.is(Blocks.COBBLESTONE)) a.add(-4);
      else if (s.is(Blocks.BASALT)) a.add(-5);
      else a.add(s.getValue(LiquidBlock.LEVEL) + (s.is(lava ? Blocks.WATER : Blocks.LAVA) ? 20 : 0));
      changes.add(a); return true;
    }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID,Map.of(
      FluidTags.WATER,List.of(Fluids.WATER.builtInRegistryHolder(),Fluids.FLOWING_WATER.builtInRegistryHolder()),
      FluidTags.LAVA,List.of(Fluids.LAVA.builtInRegistryHolder(),Fluids.FLOWING_LAVA.builtInRegistryHolder())))).apply();
    BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,Map.of(
      BlockTags.WASHED_AWAY_BY_FLUIDS,List.of(Blocks.AIR.builtInRegistryHolder(),Blocks.WATER.builtInRegistryHolder(),Blocks.LAVA.builtInRegistryHolder())))).apply();
    Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)f.get(null); var world=(TestLevel)unsafe.allocateInstance(TestLevel.class);
    var reaction=LiquidBlock.class.getDeclaredMethod("shouldSpreadLiquid",Level.class,BlockPos.class,BlockState.class); reaction.setAccessible(true);
    world.rules=new GameRules(FeatureFlags.DEFAULT_FLAGS); world.random=RandomSource.create(1729L);
    JsonArray scenarios=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray(), results=new JsonArray();
    for (var raw:scenarios) {
      var s=raw.getAsJsonObject(); boolean lava=s.get("lava").getAsBoolean(), fast=s.get("fast").getAsBoolean();
      world.rules.set(GameRules.WATER_SOURCE_CONVERSION,s.get("convert").getAsBoolean(),null);
      world.rules.set(GameRules.LAVA_SOURCE_CONVERSION,s.get("convert").getAsBoolean(),null);
      var map=EnvironmentAttributeMap.builder().set(EnvironmentAttributes.FAST_LAVA,fast).build();
      world.attrs=EnvironmentAttributeSystem.builder().addConstantLayer(map).build();
      world.blocks=new HashMap<>(); world.changes=new JsonArray(); world.lava=lava;
      for(var item:s.getAsJsonArray("blocks")) { var a=item.getAsJsonArray(); int code=a.get(3).getAsInt();
        Block liquid = code >= 20 ? (lava ? Blocks.WATER : Blocks.LAVA) : (lava ? Blocks.LAVA : Blocks.WATER);
        BlockState state=code == -2 ? Blocks.STONE.defaultBlockState() : code == -3 ? Blocks.BLUE_ICE.defaultBlockState() : code == -4 ? Blocks.SOUL_SOIL.defaultBlockState() : code == -5 ? Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.WATERLOGGED,true) : liquid.defaultBlockState().setValue(LiquidBlock.LEVEL,code >= 20 ? code-20 : code);
        world.blocks.put(new BlockPos(a.get(0).getAsInt(),a.get(1).getAsInt(),a.get(2).getAsInt()),state);
      }
      BlockPos p=BlockPos.ZERO; var block=world.getBlockState(p); var fluid=block.getFluidState();
      if(s.has("reaction")) reaction.invoke(Blocks.LAVA,world,p,block);
      else ((FlowingFluid)fluid.getType()).tick(world,p,block,fluid);
      results.add(world.changes);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(results));
  }
}
"#;

fn java_bin() -> PathBuf {
    if let Some(path) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(path).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let file = game.join("libraries").join(path);
            if file.is_file() {
                paths.push(file);
            }
        }
    }
    std::env::join_paths(paths).unwrap()
}

fn run(command: &mut Command) {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "Java fluid tick oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn liquid(kind: FluidKind, level: u8) -> FluidState {
    FluidState {
        kind,
        amount: if level == 0 || level >= 8 {
            8
        } else {
            8 - level
        },
        source: level == 0,
        falling: level >= 8,
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn scheduled_fluid_tick_matches_vanilla_in_simple_terrain() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FluidTickOracle.java");
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
    let mut seed = 41017_u32;
    let mut next = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        seed
    };
    let mut scenarios = Vec::new();
    for index in 0..1536 {
        let lava = index % 3 != 0;
        let mixed = index >= 768;
        let mut blocks = Vec::new();
        for x in -5..=5 {
            for z in -5..=5 {
                if next() % 7 != 0 {
                    blocks.push(json!([x, -1, z, -2]));
                } else if mixed && next() % 3 == 0 {
                    blocks.push(json!([x, -1, z, 20 + next() % 16]));
                }
                if x != 0 || z != 0 {
                    let value = next() % 19;
                    if value < 4 {
                        blocks.push(json!([x, 0, z, -2]));
                    } else if value < 8 {
                        let opposite = if mixed && next() % 4 == 0 { 20 } else { 0 };
                        blocks.push(json!([x, 0, z, opposite + next() % 16]));
                    }
                }
            }
        }
        blocks.push(json!([0, 0, 0, next() % 16]));
        if index % 5 == 0 {
            blocks.push(json!([0, 1, 0, next() % 16]));
        }
        scenarios.push(json!({"blocks": blocks, "lava": lava, "fast": index % 4 == 0, "convert": index % 2 == 0}));
    }
    // Explicit cross-fluid cases, including lava height and source conversion.
    for (lava, blocks) in [
        (true, vec![json!([0, 0, 0, 0]), json!([0, -1, 0, 20])]),
        (false, vec![json!([0, 0, 0, 0]), json!([0, -1, 0, 20])]),
        (
            false,
            vec![
                json!([0, 0, 0, 0]),
                json!([0, -1, 0, -2]),
                json!([0, 0, -1, -2]),
                json!([0, 0, 1, -2]),
                json!([-1, 0, 0, -2]),
                json!([1, 0, 0, 20]),
                json!([1, -1, 0, -2]),
            ],
        ),
        (
            false,
            vec![
                json!([0, 0, 0, 0]),
                json!([0, -1, 0, -2]),
                json!([0, 0, -1, -2]),
                json!([0, 0, 1, -2]),
                json!([-1, 0, 0, -2]),
                json!([1, 0, 0, 25]),
                json!([1, 1, 0, 20]),
                json!([1, -1, 0, -2]),
            ],
        ),
        (
            false,
            vec![
                json!([0, 0, 0, 0]),
                json!([-1, -1, 0, 0]),
                json!([1, -1, 0, 0]),
                json!([0, -2, 0, -2]),
            ],
        ),
    ] {
        scenarios.push(json!({"blocks": blocks, "lava": lava, "fast": false, "convert": true}));
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&scenarios).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("FluidTickOracle")
        .arg(&input)
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut differences = Vec::new();
    for (index, (scenario, java)) in scenarios
        .iter()
        .zip(expected.as_array().unwrap())
        .enumerate()
    {
        let kind = if scenario["lava"].as_bool().unwrap() {
            FluidKind::Lava
        } else {
            FluidKind::Water
        };
        let mut blocks = HashMap::new();
        for block in scenario["blocks"].as_array().unwrap() {
            let pos = IVec3::new(
                block[0].as_i64().unwrap() as i32,
                block[1].as_i64().unwrap() as i32,
                block[2].as_i64().unwrap() as i32,
            );
            let level = block[3].as_i64().unwrap();
            let other = if kind == FluidKind::Water {
                FluidKind::Lava
            } else {
                FluidKind::Water
            };
            blocks.insert(
                pos,
                if level == -2 {
                    Cell::Solid
                } else if level >= 20 {
                    Cell::Fluid(liquid(other, (level - 20) as u8))
                } else {
                    Cell::Fluid(liquid(kind, level as u8))
                },
            );
        }
        let convert = scenario["convert"].as_bool().unwrap();
        let settings = Settings {
            water_source_conversion: convert,
            lava_source_conversion: convert,
            fast_lava: scenario["fast"].as_bool().unwrap(),
        };
        let changes = fluid_sim::tick(
            &|pos| blocks.get(&pos).copied().unwrap_or(Cell::Air),
            IVec3::ZERO,
            kind,
            settings,
        );
        let actual: Vec<_> = changes
            .iter()
            .map(|change| {
                let level = match change.block {
                    ChangedBlock::Air => -1,
                    ChangedBlock::Solid(Solidified::Stone) => -2,
                    ChangedBlock::Solid(other) => panic!("unexpected solid: {other:?}"),
                    ChangedBlock::Fluid(s) => {
                        let offset = if s.kind == kind { 0 } else { 20 };
                        offset
                            + if s.source {
                                0
                            } else {
                                i32::from(8 - s.amount) + if s.falling { 8 } else { 0 }
                            }
                    }
                };
                json!([
                    change.position.x,
                    change.position.y,
                    change.position.z,
                    level
                ])
            })
            .collect();
        if actual != *java.as_array().unwrap() {
            differences.push(format!(
                "case {index}: expected {java}, actual {actual:?}, input {scenario}"
            ));
            if differences.len() == 5 {
                break;
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    eprintln!(
        "Compared {} scheduled fluid ticks with Minecraft 26.3",
        scenarios.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn lava_contact_reactions_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FluidTickOracle.java");
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
    let mut scenarios = Vec::new();
    let neighbors = [
        IVec3::Y,
        IVec3::NEG_Y,
        IVec3::NEG_Z,
        IVec3::Z,
        IVec3::NEG_X,
        IVec3::X,
    ];
    for level in 0..16 {
        for pos in neighbors {
            for code in [20, 23, -5] {
                scenarios.push(json!({"blocks": [[0, 0, 0, level], [pos.x, pos.y, pos.z, code]], "lava": true, "fast": false, "convert": false, "reaction": true}));
            }
        }
    }
    let mut seed = 0xa91317b5_u32;
    for index in 0..1024 {
        let mut blocks = vec![json!([0, 0, 0, index % 16])];
        for pos in neighbors {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let code = match (seed >> 16) % 7 {
                0 => -3,
                1 => -4,
                2 => -5,
                3 => 20,
                4 => 26,
                _ => continue,
            };
            blocks.push(json!([pos.x, pos.y, pos.z, code]));
        }
        scenarios.push(json!({"blocks": blocks, "lava": true, "fast": false, "convert": false, "reaction": true}));
    }
    let input = temp.path().join("reactions.json");
    let output = temp.path().join("results.json");
    std::fs::write(&input, serde_json::to_vec(&scenarios).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("FluidTickOracle")
        .arg(&input)
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut differences = Vec::new();
    for (index, (scenario, java)) in scenarios
        .iter()
        .zip(expected.as_array().unwrap())
        .enumerate()
    {
        let mut blocks = HashMap::new();
        let mut center = liquid(FluidKind::Lava, 0);
        for block in scenario["blocks"].as_array().unwrap() {
            let pos = IVec3::new(
                block[0].as_i64().unwrap() as i32,
                block[1].as_i64().unwrap() as i32,
                block[2].as_i64().unwrap() as i32,
            );
            let code = block[3].as_i64().unwrap();
            if pos == IVec3::ZERO {
                center = liquid(FluidKind::Lava, code as u8);
                continue;
            }
            blocks.insert(
                pos,
                match code {
                    -3 => LavaNeighbor::BlueIce,
                    -4 => LavaNeighbor::SoulSoil,
                    -5 | 20..=35 => LavaNeighbor::Water,
                    _ => LavaNeighbor::Other,
                },
            );
        }
        let reaction = fluid_sim::lava_reaction(
            &|pos| blocks.get(&pos).copied().unwrap_or(LavaNeighbor::Other),
            IVec3::ZERO,
            center,
        );
        let actual = match reaction {
            None => vec![],
            Some(Solidified::Obsidian) => vec![json!([0, 0, 0, -3])],
            Some(Solidified::Cobblestone) => vec![json!([0, 0, 0, -4])],
            Some(Solidified::Basalt) => vec![json!([0, 0, 0, -5])],
            Some(Solidified::Stone) => unreachable!(),
        };
        if actual != *java.as_array().unwrap() {
            differences.push(format!(
                "case {index}: expected {java}, actual {actual:?}, input {scenario}"
            ));
            if differences.len() == 5 {
                break;
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    eprintln!(
        "Compared {} lava contact reactions with Minecraft 26.3",
        scenarios.len()
    );
}
