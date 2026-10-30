//! Compare basic block destroy progress with the locally installed Java 26.3 code.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_persistence::nbt::Tag;
use native_session::mining::{Material, basic_progress, mining_properties, tool_progress};
use native_session::{
    GameSession, LocalSession, PlayerCommand,
    inventory::{ItemStack, PlayerInventory, PlayerSlot},
};
use native_world::{Block, World};
use serde_json::Value;

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ore_experience_and_silk_touch_match_vanilla() {
    use native_core::random::LegacyRandom;
    use native_resources::{enchantments::OxygenEnchantments, packs::ResourceStack};
    use serde_json::json;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let registry = OxygenEnchantments::load(&mut ResourceStack::open(&game).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("OreExperienceOracle.java");
    std::fs::write(&source, include_str!("java/OreExperienceOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let ores = [
        "coal_ore",
        "deepslate_coal_ore",
        "iron_ore",
        "deepslate_iron_ore",
        "copper_ore",
        "deepslate_copper_ore",
        "gold_ore",
        "deepslate_gold_ore",
        "diamond_ore",
        "deepslate_diamond_ore",
        "emerald_ore",
        "deepslate_emerald_ore",
        "lapis_ore",
        "deepslate_lapis_ore",
        "redstone_ore",
        "deepslate_redstone_ore",
        "nether_gold_ore",
        "nether_quartz_ore",
    ];
    let mut cases = Vec::new();
    for seed in -50..50 {
        for block in ores {
            for silk in [false, true] {
                cases.push(json!({"seed":seed,"block":format!("minecraft:{block}"),"silk":silk}));
            }
        }
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("OreExperienceOracle")
        .arg(&input)
        .arg(&output));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), cases.len());
    let tool = ItemStack::new(
        "minecraft:iron_pickaxe".into(),
        1,
        1,
        Tag::Compound(vec![(
            "minecraft:enchantments".into(),
            Tag::Compound(vec![("minecraft:silk_touch".into(), Tag::Int(1))]),
        )]),
    )
    .unwrap();
    for (case, row) in cases.iter().zip(rows) {
        let mut random = LegacyRandom::new(case["seed"].as_i64().unwrap());
        let block = case["block"].as_str().unwrap();
        assert_eq!(
            native_session::mining::ore_experience_range(block),
            Some((
                row[0].as_i64().unwrap() as i32,
                row[1].as_i64().unwrap() as i32
            )),
            "{case}"
        );
        let item = case["silk"].as_bool().unwrap().then_some(&tool);
        assert_eq!(
            native_session::mining::ore_experience(block, item, Some(&registry), &mut random)
                .unwrap(),
            Some(row[2].as_i64().unwrap() as i32),
            "{case}"
        );
        assert_eq!(random.next_int(), row[3].as_i64().unwrap() as i32, "{case}");
    }
    eprintln!(
        "Compared {} ore experience cases with Minecraft 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ore_loot_counts_and_random_stream_match_vanilla() {
    use native_core::random::LegacyRandom;
    use native_resources::{
        loot::{BasicTool, BlockLootTables},
        packs::{ResourceId, ResourceStack},
    };
    use serde_json::json;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let tables = BlockLootTables::load(&mut ResourceStack::open(&game).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("OreLootOracle.java");
    std::fs::write(&source, include_str!("java/OreLootOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut cases = Vec::new();
    for seed in -50..50 {
        for block in [
            "coal_ore",
            "iron_ore",
            "diamond_ore",
            "copper_ore",
            "lapis_ore",
            "redstone_ore",
            "nether_gold_ore",
        ] {
            for level in [0, 1, 2, 3, 10, 255] {
                for silk in [false, true] {
                    cases.push(json!({"seed":seed,"block":block,"level":level,"silk":silk}));
                }
            }
        }
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("OreLootOracle")
        .arg(&input)
        .arg(&output));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), cases.len());
    for (case, row) in cases.iter().zip(rows) {
        let mut random = LegacyRandom::new(case["seed"].as_i64().unwrap());
        let mut enchants = Vec::new();
        let level = case["level"].as_u64().unwrap() as u32;
        if level > 0 {
            enchants.push((ResourceId::parse("minecraft:fortune").unwrap(), level));
        }
        if case["silk"].as_bool().unwrap() {
            enchants.push((ResourceId::parse("minecraft:silk_touch").unwrap(), 1));
        }
        let tool = BasicTool {
            item: Some("minecraft:iron_pickaxe"),
            count: 1,
            enchantments: &enchants,
        };
        let block = format!("minecraft:{}", case["block"].as_str().unwrap());
        let drops = tables
            .drops_with_random(&block, tool, Some(&mut random))
            .unwrap();
        assert_eq!(drops.len(), 1, "{case}");
        assert_eq!(drops[0].count, row[0].as_u64().unwrap() as u32, "{case}");
        assert_eq!(random.next_int(), row[1].as_i64().unwrap() as i32, "{case}");
    }
    eprintln!(
        "Compared {} ore loot cases with Minecraft 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn enchanted_gravel_loot_conditions_match_vanilla() {
    use native_core::random::LegacyRandom;
    use native_resources::{
        loot::{BasicTool, BlockLootTables},
        packs::{ResourceId, ResourceStack},
    };
    use serde_json::json;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let tables = BlockLootTables::load(&mut ResourceStack::open(&game).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("EnchantedLootOracle.java");
    std::fs::write(&source, include_str!("java/EnchantedLootOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut cases = Vec::new();
    for seed in -100..100 {
        for fortune in [0, 1, 2, 3, 10, 255] {
            for silk in [0, 1] {
                cases.push(json!({"seed":seed,"fortune":fortune,"silk":silk}));
            }
        }
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("EnchantedLootOracle")
        .arg(&input)
        .arg(&output));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), cases.len());
    for (case, row) in cases.iter().zip(rows) {
        let mut random = LegacyRandom::new(case["seed"].as_i64().unwrap());
        let mut enchants = Vec::new();
        for name in ["fortune", "silk"] {
            let level = case[name].as_u64().unwrap() as u32;
            if level != 0 {
                enchants.push((
                    ResourceId::parse(if name == "silk" {
                        "minecraft:silk_touch"
                    } else {
                        "minecraft:fortune"
                    })
                    .unwrap(),
                    level,
                ));
            }
        }
        let tool = BasicTool {
            item: Some("minecraft:iron_shovel"),
            count: 1,
            enchantments: &enchants,
        };
        let drops = tables
            .drops_with_random("minecraft:gravel", tool, Some(&mut || random.next_float()))
            .unwrap();
        assert_eq!(drops[0].item.path, row[0].as_str().unwrap(), "{case}");
        assert_eq!(random.next_int(), row[1].as_i64().unwrap() as i32, "{case}");
    }
    eprintln!(
        "Compared {} enchanted gravel cases with Minecraft 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn unbreaking_effect_matches_vanilla_and_consumes_the_same_random_values() {
    use native_core::random::LegacyRandom;
    use native_resources::{
        enchantments::OxygenEnchantments,
        packs::{ResourceId, ResourceStack},
    };
    use serde_json::json;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let registry = OxygenEnchantments::load(&mut ResourceStack::open(&game).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("UnbreakingOracle.java");
    std::fs::write(&source, include_str!("java/UnbreakingOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut cases = Vec::new();
    for seed in -50..50 {
        for armor in [false, true] {
            for level in [1, 2, 3, 10, 255] {
                for amount in [1, 2, 10, 40] {
                    cases.push(
                        json!({"seed": seed, "armor": armor, "level": level, "amount": amount}),
                    );
                }
            }
        }
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("UnbreakingOracle")
        .arg(&input)
        .arg(&output));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), cases.len());
    let id = ResourceId::parse("minecraft:unbreaking").unwrap();
    for (case, row) in cases.iter().zip(rows) {
        let mut random = LegacyRandom::new(case["seed"].as_i64().unwrap());
        let item = if case["armor"].as_bool().unwrap() {
            "minecraft:iron_helmet"
        } else {
            "minecraft:iron_pickaxe"
        };
        let (value, active) = registry
            .apply_item_damage(
                &id,
                case["level"].as_u64().unwrap() as u32,
                item,
                case["amount"].as_f64().unwrap() as f32,
                &mut || random.next_float(),
            )
            .unwrap();
        assert!(active);
        assert_eq!(
            value.to_bits(),
            row[0].as_i64().unwrap() as i32 as u32,
            "{case}"
        );
        assert_eq!(random.next_int(), row[1].as_i64().unwrap() as i32, "{case}");
    }
    eprintln!(
        "Compared {} Unbreaking cases with Minecraft 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn mining_effects_and_attributes_match_vanilla() {
    use native_session::mining::{MiningContext, destroy_speed, tool_progress_with_context};
    use serde_json::json;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("MiningEffectsOracle.java");
    std::fs::write(&source, include_str!("java/MiningEffectsOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let effects = [-1_i32, 0, 1, 2, 10, 255];
    let mut cases = Vec::new();
    for i in 0..2880 {
        cases.push(json!({
            "stone": i % 2 == 0, "tool": (i / 2) % 2 == 0,
            "ground": (i / 4) % 2 == 0, "water": (i / 8) % 2 == 0,
            "efficiency": ([-1.0, 0.0, 2.0, 25.0, 2000.0][(i / 16) % 5]),
            "break_speed": ([-1.0, 0.5, 1.0, 3.0][(i / 80) % 4]),
            "submerged": ([-1.0, 0.2, 1.0, 30.0][(i / 320) % 4]),
            "haste": effects[(i / 3) % effects.len()],
            "conduit": effects[(i / 11) % effects.len()],
            "fatigue": effects[(i / 23) % effects.len()],
        }));
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("MiningEffectsOracle")
        .arg(&input)
        .arg(&output));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), cases.len());
    for (i, (case, row)) in cases.iter().zip(&rows).enumerate() {
        let material = if case["stone"].as_bool().unwrap() {
            Material::Stone
        } else {
            Material::Dirt
        };
        let tool = case["tool"].as_bool().unwrap().then(|| {
            ItemStack::new("minecraft:iron_pickaxe".into(), 1, 1, Tag::Compound(vec![])).unwrap()
        });
        let status = |key: &str| case[key].as_i64().and_then(|v| u8::try_from(v).ok());
        let context = MiningContext {
            on_ground: case["ground"].as_bool().unwrap(),
            eyes_in_water: case["water"].as_bool().unwrap(),
            mining_efficiency: case["efficiency"].as_f64().unwrap().clamp(0.0, 1024.0),
            block_break_speed: case["break_speed"].as_f64().unwrap().clamp(0.0, 1024.0),
            submerged_mining_speed: case["submerged"].as_f64().unwrap().clamp(0.0, 20.0),
            haste: status("haste"),
            conduit_power: status("conduit"),
            mining_fatigue: status("fatigue"),
        };
        let properties = mining_properties(material, tool.as_ref()).unwrap();
        assert_eq!(
            destroy_speed(properties.speed, context).to_bits(),
            row[0].as_i64().unwrap() as i32 as u32,
            "destroy speed case {i}: {case}"
        );
        assert_eq!(
            tool_progress_with_context(material, tool.as_ref(), context)
                .unwrap()
                .to_bits(),
            row[1].as_i64().unwrap() as i32 as u32,
            "progress case {i}: {case}"
        );
    }
    println!(
        "Compared {} mining attribute/effect cases with Minecraft 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn falling_block_render_origin_matches_java_pose_stack() {
    use native_session::falling_block::FallingBlockVisual;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FallingRenderOracle.java");
    std::fs::write(&source, r#"
import java.nio.file.*;
import com.google.gson.*;
import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.util.Mth;
public class FallingRenderOracle {
  public static void main(String[] args) throws Exception {
    var input=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray(); var result=new JsonArray();
    for (var entry:input) { var c=entry.getAsJsonObject(); var a=c.getAsJsonArray("previous"); var b=c.getAsJsonArray("position"); var o=c.getAsJsonArray("origin"); float t=c.get("partial").getAsFloat();
      var pose=new PoseStack();
      pose.translate(Mth.lerp(t,a.get(0).getAsDouble(),b.get(0).getAsDouble())-o.get(0).getAsDouble(),Mth.lerp(t,a.get(1).getAsDouble(),b.get(1).getAsDouble())-o.get(1).getAsDouble(),Mth.lerp(t,a.get(2).getAsDouble(),b.get(2).getAsDouble())-o.get(2).getAsDouble());
      pose.translate(-0.5,0.0,-0.5); var m=pose.last().pose(); var row=new JsonArray(); row.add(m.m30()); row.add(m.m31()); row.add(m.m32()); result.add(row);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(result));
  }
}
"#).unwrap();
    let cases = [
        (
            glam::DVec3::new(0.5, 4.0, 0.5),
            glam::DVec3::new(0.5, 3.4, 0.5),
            glam::IVec3::ZERO,
            0.5_f32,
        ),
        (
            glam::DVec3::new(-16000000.5, 80.0, 16000000.5),
            glam::DVec3::new(-16000000.5, 79.96, 16000000.5),
            glam::IVec3::new(-16000000, 0, 16000000),
            0.25_f32,
        ),
        (
            glam::DVec3::new(8.5, 1.0, -9.5),
            glam::DVec3::new(9.5, 0.0, -9.5),
            glam::IVec3::new(0, 0, -16),
            1.0_f32,
        ),
    ];
    let input: Vec<_>=cases.iter().map(|(a,b,o,t)| serde_json::json!({"previous":a.to_array(),"position":b.to_array(),"origin":o.to_array(),"partial":t})).collect();
    let inp = temp.path().join("input.json");
    let out = temp.path().join("output.json");
    std::fs::write(&inp, serde_json::to_vec(&input).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FallingRenderOracle")
        .arg(inp)
        .arg(&out));
    let expected: Vec<[f32; 3]> = serde_json::from_slice(&std::fs::read(out).unwrap()).unwrap();
    for (index, (previous_position, position, origin, partial)) in cases.into_iter().enumerate() {
        let visual = FallingBlockVisual {
            previous_position,
            position,
            kind: native_session::falling_block::FallingKind::Sand,
        };
        let actual = visual.model_origin(origin, partial).as_vec3();
        assert!(
            (actual - glam::Vec3::from_array(expected[index]))
                .abs()
                .max_element()
                < 1.0e-5,
            "case {index}"
        );
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn java_saved_tick_duplicates_and_new_schedule_order() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SavedTickOrderOracle.java");
    std::fs::write(&source, r#"
import java.util.*;
import java.io.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.core.BlockPos;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.ticks.*;
public class SavedTickOrderOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var pos=new BlockPos(1,2,3);
    for(var names:List.of(List.of("button","sand"),List.of("sand","button"))) {
      var mixed=new LevelChunkTicks<String>(List.of(
        new SavedTick<String>(names.get(0),pos,0,TickPriority.NORMAL),
        new SavedTick<String>(names.get(1),pos.offset(1,0,0),0,TickPriority.NORMAL)));
      mixed.unpack(0L);
      var firstMixed=mixed.poll(); var secondMixed=mixed.poll();
      if(firstMixed==null || secondMixed==null || !firstMixed.type().equals(names.get(0)) ||
         !secondMixed.type().equals(names.get(1)) || mixed.poll()!=null)
        throw new AssertionError("mixed block tick order differs from saved sub-tick order");
    }
    var saved=new SavedTick<String>("water",pos,3,TickPriority.NORMAL);
    var ticks=new LevelChunkTicks<String>(List.of(saved,saved));
    ticks.unpack(0L);
    ticks.schedule(new ScheduledTick<String>("water",pos,1,TickPriority.HIGH,0));
    var first=ticks.poll(); var second=ticks.poll();
    if(first==null || second==null || first.subTickOrder()!=-2 || second.subTickOrder()!=-1 || ticks.poll()!=null) throw new AssertionError("saved duplicates or deduplication differ");
    var partial=new LevelChunkTicks<String>(List.of(saved,saved)); partial.unpack(0L);
    partial.poll();
    partial.schedule(new ScheduledTick<String>("water",pos,1,TickPriority.HIGH,7));
    var inserted=partial.poll(); var original=partial.poll();
    if(inserted==null || original==null || inserted.subTickOrder()!=7 || original.subTickOrder()!=-1 || partial.poll()!=null)
      throw new AssertionError("scheduling after polling one stored duplicate differs");
    var packing=new LevelChunkTicks<String>();
    packing.schedule(new ScheduledTick<String>("sand",pos,3,TickPriority.NORMAL,1));
    packing.schedule(new ScheduledTick<String>("gravel",pos.offset(1,0,0),8,TickPriority.HIGH,-1));
    var packed=packing.pack(5L);
    if(packed.size()!=2 || !packed.get(0).type().equals("gravel") || packed.get(0).delay()!=3 ||
       !packed.get(1).type().equals("sand") || packed.get(1).delay()!=-2)
      throw new AssertionError("saved tick ordering or relative delay differs");
    var input=new DataInputStream(Files.newInputStream(Path.of(args[0])));
    var root=NbtIo.read((DataInput)input); input.close();
    var actual=SavedTick.codec(BuiltInRegistries.BLOCK.byNameCodec()).listOf().parse(NbtOps.INSTANCE,root.get("block_ticks")).getOrThrow();
    if(actual.size()!=5 || actual.get(0).type()!=Blocks.GRAVEL || actual.get(0).delay()!=3 || actual.get(0).priority()!=TickPriority.HIGH || !actual.get(0).pos().equals(new BlockPos(-15,-60,33)) ||
       actual.get(1).type()!=Blocks.STONE_BUTTON || actual.get(1).delay()!=7 || actual.get(1).priority()!=TickPriority.NORMAL || !actual.get(1).pos().equals(new BlockPos(-14,-60,33)) ||
       actual.get(2).type()!=Blocks.SAND || actual.get(2).delay()!=-2 || actual.get(2).priority()!=TickPriority.NORMAL || !actual.get(2).pos().equals(new BlockPos(-14,-61,33)) ||
       actual.get(3).type()!=Blocks.DIRT_PATH || actual.get(3).delay()!=-3 || actual.get(3).priority()!=TickPriority.NORMAL || !actual.get(3).pos().equals(new BlockPos(-13,-61,33)) ||
       actual.get(4).type()!=Blocks.FARMLAND || actual.get(4).delay()!=5 || actual.get(4).priority()!=TickPriority.HIGH || !actual.get(4).pos().equals(new BlockPos(-12,-61,33)))
      throw new AssertionError("Rust NBT tick draft differs from Java codec");
    var fluid=SavedTick.codec(BuiltInRegistries.FLUID.byNameCodec()).listOf().parse(NbtOps.INSTANCE,root.get("fluid_ticks")).getOrThrow();
    if(fluid.size()!=2 || fluid.get(0).type()!=Fluids.FLOWING_LAVA || fluid.get(0).delay()!=3 || fluid.get(0).priority()!=TickPriority.HIGH || !fluid.get(0).pos().equals(new BlockPos(-15,-60,33)) ||
       fluid.get(1).type()!=Fluids.WATER || fluid.get(1).delay()!=-2 || fluid.get(1).priority()!=TickPriority.NORMAL || !fluid.get(1).pos().equals(new BlockPos(-14,-61,33)))
      throw new AssertionError("Rust NBT fluid tick draft differs from Java codec");
  }
}

"#).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    let chunk = native_persistence::nbt::NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            ("xPos".into(), Tag::Int(-1)),
            ("zPos".into(), Tag::Int(2)),
            ("Status".into(), Tag::String("minecraft:full".into())),
        ]),
    };
    let mut draft = native_persistence::edit::draft_chunk_blocks(
        &chunk,
        native_persistence::world::ChunkPos { x: -1, z: 2 },
        &[],
    )
    .unwrap();
    draft
        .replace_block_ticks(&[
            native_world::loaded::SavedBlockTick {
                block: "minecraft:gravel".into(),
                position: [-15, -60, 33],
                delay: 3,
                priority: -1,
            },
            native_world::loaded::SavedBlockTick {
                block: "minecraft:stone_button".into(),
                position: [-14, -60, 33],
                delay: 7,
                priority: 0,
            },
            native_world::loaded::SavedBlockTick {
                block: "minecraft:sand".into(),
                position: [-14, -61, 33],
                delay: -2,
                priority: 0,
            },
            native_world::loaded::SavedBlockTick {
                block: "minecraft:dirt_path".into(),
                position: [-13, -61, 33],
                delay: -3,
                priority: 0,
            },
            native_world::loaded::SavedBlockTick {
                block: "minecraft:farmland".into(),
                position: [-12, -61, 33],
                delay: 5,
                priority: -1,
            },
        ])
        .unwrap();
    draft
        .replace_fluid_ticks(&[
            native_world::loaded::SavedFluidTick {
                fluid: "minecraft:flowing_lava".into(),
                position: [-15, -60, 33],
                delay: 3,
                priority: -1,
            },
            native_world::loaded::SavedFluidTick {
                fluid: "minecraft:water".into(),
                position: [-14, -61, 33],
                delay: -2,
                priority: 0,
            },
        ])
        .unwrap();
    let nbt_path = temp.path().join("tick-draft.nbt");
    std::fs::write(
        &nbt_path,
        native_persistence::nbt::write(draft.document()).unwrap(),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SavedTickOrderOracle")
        .arg(nbt_path));
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn soil_placement_push_and_spectator_filter_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SoilPlacementOracle.java");
    std::fs::write(&source, include_str!("java/SoilPlacementOracle.java")).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SoilPlacementOracle"));
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn falling_block_clipping_matches_java_shapes() {
    use native_world::movement::{BoundingBox, CollisionShape, resolve_movement};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FallingCollisionOracle.java");
    std::fs::write(&source, r#"
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.phys.*;
import net.minecraft.world.phys.shapes.*;
public class FallingCollisionOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var method=Entity.class.getDeclaredMethod("collideWithShapes",Vec3.class,AABB.class,List.class); method.setAccessible(true);
    var input=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray(); var output=new JsonArray();
    var shapes=List.of(Shapes.create(new AABB(-10,0,-10,10,1,10)),Shapes.create(new AABB(2,1,-2,3,5,2)));
    var dimensions=EntityTypes.FALLING_BLOCK.getDimensions();
    for(var entry:input) { var c=entry.getAsJsonObject(); var p=c.getAsJsonArray("p"); var v=c.getAsJsonArray("v");
      double x=p.get(0).getAsDouble(), y=p.get(1).getAsDouble(), z=p.get(2).getAsDouble();
      var moved=(Vec3)method.invoke(null,new Vec3(v.get(0).getAsDouble(),v.get(1).getAsDouble(),v.get(2).getAsDouble()),dimensions.makeBoundingBox(new Vec3(x,y,z)),shapes);
      JsonArray row=new JsonArray(); row.add(moved.x); row.add(moved.y); row.add(moved.z); output.add(row);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(output));
  }
}
"#).unwrap();
    let mut cases = Vec::new();
    for y in [1.0, 1.5, 3.0, 6.0] {
        for x in [0.5, 1.0, 1.5] {
            for velocity in [
                glam::DVec3::new(3.0, -4.0, 0.0),
                glam::DVec3::new(-1.0, -0.1, 0.5),
                glam::DVec3::new(0.3, 1.0, -2.0),
            ] {
                cases.push((glam::DVec3::new(x, y, 0.5), velocity));
            }
        }
    }
    let input: Vec<_> = cases
        .iter()
        .map(|(p, v)| serde_json::json!({"p":p.to_array(),"v":v.to_array()}))
        .collect();
    let input_path = temp.path().join("collision-input.json");
    let result = temp.path().join("collision-output.json");
    std::fs::write(&input_path, serde_json::to_vec(&input).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FallingCollisionOracle")
        .arg(input_path)
        .arg(&result));
    let expected: Vec<[f64; 3]> = serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
    let shapes = [
        BoundingBox {
            min: glam::DVec3::new(-10.0, 0.0, -10.0),
            max: glam::DVec3::new(10.0, 1.0, 10.0),
        },
        BoundingBox {
            min: glam::DVec3::new(2.0, 1.0, -2.0),
            max: glam::DVec3::new(3.0, 5.0, 2.0),
        },
    ]
    .map(|shape| CollisionShape::new(vec![shape]));
    for (index, (position, velocity)) in cases.into_iter().enumerate() {
        let bounds = native_session::falling_block::AirborneMotion {
            position,
            velocity,
            age: 0,
            no_gravity: false,
        }
        .bounds();
        let actual = resolve_movement(bounds, velocity, false, 0.0, &shapes);
        assert!(
            (actual - glam::DVec3::from_array(expected[index]))
                .abs()
                .max_element()
                < 1.0e-10,
            "case {index}"
        );
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn airborne_falling_block_motion_matches_java() {
    use native_session::falling_block::AirborneMotion;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FallingMotionOracle.java");
    std::fs::write(&source, r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.MoverType;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.phys.Vec3;
public class FallingMotionOracle {
  static Field position, block;
  static class World extends ClientLevel { World() { super(null,null,null,null,0,0,null,false,0L,0); } }
  static class Falling extends FallingBlockEntity {
    Vec3 velocity; Level world; boolean noGravity;
    Falling() { super(null,null); }
    void pos(Vec3 p) { try { position.set(this,p); } catch(Exception e) { throw new RuntimeException(e); } }
    @Override public Level level() { return world; }
    @Override public boolean isNoGravity() { return noGravity; }
    @Override public Vec3 getDeltaMovement() { return velocity; }
    @Override public void setDeltaMovement(Vec3 v) { velocity=v; }
    @Override protected void applyEffectsFromBlocks() { }
    @Override protected void handlePortal() { }
    @Override public void setRequiresPrecisePosition(boolean v) { }
    @Override public void move(MoverType type, Vec3 v) { pos(new Vec3(getX()+v.x,getY()+v.y,getZ()+v.z)); }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    position=Entity.class.getDeclaredField("position"); position.setAccessible(true);
    block=FallingBlockEntity.class.getDeclaredField("blockState"); block.setAccessible(true);
    Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var input=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray(); var output=new JsonArray();
    for(var row:input) {
      var c=row.getAsJsonObject(); var p=c.getAsJsonArray("p"); var v=c.getAsJsonArray("v");
      var e=(Falling)unsafe.allocateInstance(Falling.class); e.world=(World)unsafe.allocateInstance(World.class);
      block.set(e,Blocks.SAND.defaultBlockState()); e.pos(new Vec3(p.get(0).getAsDouble(),p.get(1).getAsDouble(),p.get(2).getAsDouble()));
      e.velocity=new Vec3(v.get(0).getAsDouble(),v.get(1).getAsDouble(),v.get(2).getAsDouble()); e.noGravity=c.get("no_gravity").getAsBoolean(); e.time=c.get("age").getAsInt();
      JsonArray ticks=new JsonArray(); for(int i=0;i<40;i++) { e.tick(); JsonArray a=new JsonArray(); a.add(e.getX()); a.add(e.getY()); a.add(e.getZ()); a.add(e.velocity.x); a.add(e.velocity.y); a.add(e.velocity.z); a.add(e.time); ticks.add(a); } output.add(ticks);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(output));
  }
}
"#).unwrap();
    let cases = [
        (
            glam::DVec3::new(0.5, 64.0, -1.5),
            glam::DVec3::ZERO,
            0,
            false,
        ),
        (
            glam::DVec3::new(-3.5, 100.0, 2.5),
            glam::DVec3::new(0.3, -0.1, -0.2),
            i32::MAX - 2,
            false,
        ),
        (
            glam::DVec3::new(1.0, 1.0, 1.0),
            glam::DVec3::new(0.1, 0.2, 0.3),
            42,
            true,
        ),
    ];
    let input: Vec<_> = cases.iter().map(|(p,v,age,no_gravity)| serde_json::json!({"p":p.to_array(),"v":v.to_array(),"age":age,"no_gravity":no_gravity})).collect();
    let input_path = temp.path().join("input.json");
    let result = temp.path().join("result.json");
    std::fs::write(&input_path, serde_json::to_vec(&input).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FallingMotionOracle")
        .arg(input_path)
        .arg(&result));
    let rows: Vec<Vec<Vec<Value>>> =
        serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
    for (index, (position, velocity, age, no_gravity)) in cases.into_iter().enumerate() {
        let mut motion = AirborneMotion {
            position,
            velocity,
            age,
            no_gravity,
        };
        for row in &rows[index] {
            motion.tick_in_air();
            for (actual, expected) in motion
                .position
                .to_array()
                .into_iter()
                .chain(motion.velocity.to_array())
                .zip(row)
            {
                assert!((actual - expected.as_f64().unwrap()).abs() < 1.0e-10);
            }
            assert_eq!(i64::from(motion.age), row[6].as_i64().unwrap());
        }
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn unbreakable_block_registry_matches_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("BedrockHardness.java");
    let result = temp.path().join("hardness.txt");
    std::fs::write(
        &source,
        r#"
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.GameMasterBlock;
import net.minecraft.world.level.block.FallingBlock;
import net.minecraft.world.level.block.Blocks;
public class BedrockHardness {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    if (!(Blocks.GRAVEL instanceof FallingBlock)) throw new AssertionError("gravel is not a falling block");
    var delay=FallingBlock.class.getDeclaredMethod("getDelayAfterPlace"); delay.setAccessible(true);
    if (((Integer)delay.invoke(Blocks.GRAVEL)) != 2) throw new AssertionError("gravel tick delay differs");
    var lines = new java.util.ArrayList<String>();
    for (var block : BuiltInRegistries.BLOCK) {
      float hardness = block.defaultBlockState().getDestroySpeed(null, BlockPos.ZERO);
      lines.add(BuiltInRegistries.BLOCK.getKey(block).toString() + " " + hardness + " " + (block instanceof GameMasterBlock) + " " + FallingBlock.isFree(block.defaultBlockState()));
    }
    Files.write(Path.of(args[0]), lines);
  }
}
"#,
    )
    .unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BedrockHardness")
        .arg(&result));
    let java = std::fs::read_to_string(result).unwrap();
    for row in java.lines() {
        let mut fields = row.split_whitespace();
        let name = fields.next().unwrap();
        let hardness = fields.next().unwrap();
        let game_master = fields.next().unwrap().parse::<bool>().unwrap();
        let free = fields.next().unwrap().parse::<bool>().unwrap();
        if let Some(actual) = native_session::falling_block::known_free_below(name, false) {
            assert_eq!(actual, free, "falling block support: {name}");
        }
        assert_eq!(
            native_session::mining::requires_game_master_permission(name),
            game_master,
            "game-master classification: {name}"
        );
        let unbreakable = hardness.parse::<f32>().unwrap() < 0.0;
        if name == "minecraft:sand" || name == "minecraft:red_sand" {
            assert_eq!(Material::Sand.hardness(), hardness.parse::<f32>().unwrap());
        }
        if name == "minecraft:gravel" {
            assert_eq!(
                Material::Gravel.hardness(),
                hardness.parse::<f32>().unwrap()
            );
        }
        assert_eq!(
            Material::from_block_name(name) == Some(Material::Unbreakable),
            unbreakable,
            "{name}"
        );
    }
}

const ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.tags.TagKey;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.entity.EntityEquipment;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.ai.attributes.AttributeInstance;
import net.minecraft.world.entity.ai.attributes.Attributes;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.material.Fluid;

public class MiningOracle {
    static class TestPlayer extends Player {
        boolean grounded, underwater;
        AttributeInstance submerged;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean onGround() { return grounded; }
        @Override public boolean isEyeInFluid(TagKey<Fluid> tag) { return underwater; }
        @Override public boolean hasEffect(Holder<MobEffect> effect) { return false; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) { return 1.0; }
        @Override public AttributeInstance getAttribute(Holder<Attribute> attribute) { return submerged; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Items.STONE.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
        Field unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        TestPlayer player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        Inventory inventory = new Inventory(player, new EntityEquipment());
        Field inv = Player.class.getDeclaredField("inventory"); inv.setAccessible(true); inv.set(player, inventory);
        player.submerged = new AttributeInstance(Attributes.SUBMERGED_MINING_SPEED, value -> {});
        player.submerged.setBaseValue(0.2);
        var states = new net.minecraft.world.level.block.state.BlockState[] {
            Blocks.GRASS_BLOCK.defaultBlockState(), Blocks.DIRT.defaultBlockState(),
            Blocks.STONE.defaultBlockState(), Blocks.STONE_BRICKS.defaultBlockState(),
            Blocks.OAK_PLANKS.defaultBlockState(), Blocks.SPRUCE_PLANKS.defaultBlockState(),
            Blocks.BIRCH_PLANKS.defaultBlockState(), Blocks.JUNGLE_PLANKS.defaultBlockState(),
            Blocks.ACACIA_PLANKS.defaultBlockState(), Blocks.DARK_OAK_PLANKS.defaultBlockState(),
            Blocks.MANGROVE_PLANKS.defaultBlockState(), Blocks.CHERRY_PLANKS.defaultBlockState(),
            Blocks.PALE_OAK_PLANKS.defaultBlockState(), Blocks.BAMBOO_PLANKS.defaultBlockState(),
            Blocks.CRIMSON_PLANKS.defaultBlockState(), Blocks.WARPED_PLANKS.defaultBlockState(),
            Blocks.POPLAR_PLANKS.defaultBlockState(),
            Blocks.CRAFTING_TABLE.defaultBlockState(),
            Blocks.COARSE_DIRT.defaultBlockState(), Blocks.ROOTED_DIRT.defaultBlockState(),
            Blocks.PODZOL.defaultBlockState(), Blocks.MYCELIUM.defaultBlockState(),
            Blocks.DIRT_PATH.defaultBlockState(), Blocks.FARMLAND.defaultBlockState(),
            Blocks.STONE_BUTTON.defaultBlockState()
        };
        JsonArray output = new JsonArray();
        for (int i=0; i<states.length; i++) for (boolean ground : new boolean[]{false,true})
            for (boolean water : new boolean[]{false,true}) for (int holding : new int[]{0,1,2}) {
                player.grounded=ground; player.underwater=water;
                ItemStack hand=holding==0 ? ItemStack.EMPTY : new ItemStack(Items.STONE,1);
                if(holding==2) hand.set(DataComponents.MAX_STACK_SIZE,99);
                inventory.setItem(0, hand);
                var state = states[i];
                float hardness=state.getDestroySpeed(null, BlockPos.ZERO);
                float progress=state.getDestroyProgress(player, null, BlockPos.ZERO);
                JsonArray row = new JsonArray();
                row.add(i); row.add(ground); row.add(water); row.add(holding);
                row.add(Float.floatToRawIntBits(hardness));
                row.add(state.requiresCorrectToolForDrops());
                row.add(Float.floatToRawIntBits(progress));
                float accumulated=0.0f; int ticks=0;
                JsonArray stages = new JsonArray();
                while (accumulated < 1.0f && ticks < 10000) {
                    accumulated += progress; ticks++;
                    if (ground && !water) stages.add(accumulated < 1.0f ? (int)(accumulated * 10.0f) : -1);
                }
                row.add(ticks); row.add(stages); output.add(row);
            }
        Files.writeString(Path.of(args[0]), new Gson().toJson(output));
    }
}
"#;

// The data generator exposes the real 26.3 item components but leaves block
// tags unbound. Resolve those rule tags from the same installed JAR before
// invoking the original ItemStack and BlockState methods.
const TOOL_ORACLE: &str = r##"
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import java.util.zip.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.TagKey;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.entity.EntityEquipment;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.ai.attributes.AttributeInstance;
import net.minecraft.world.entity.ai.attributes.Attributes;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.component.Tool;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.material.Fluid;

public class ToolMiningOracle {
    static class TestPlayer extends Player {
        boolean grounded, underwater;
        AttributeInstance submerged;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean onGround() { return grounded; }
        @Override public boolean isEyeInFluid(TagKey<Fluid> tag) { return underwater; }
        @Override public boolean hasEffect(Holder<MobEffect> effect) { return false; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) {
            return attribute == Attributes.MINING_EFFICIENCY ? 0.0 : 1.0;
        }
        @Override public AttributeInstance getAttribute(Holder<Attribute> attribute) { return submerged; }
    }
    static Set<String> blockTag(ZipFile jar, String name, Set<String> visiting) throws Exception {
        if (!visiting.add(name)) throw new IllegalStateException("cyclic tag " + name);
        var id = Identifier.parse(name);
        var entry = jar.getEntry("data/" + id.getNamespace() + "/tags/block/" + id.getPath() + ".json");
        if (entry == null) throw new IllegalStateException("missing block tag " + name);
        var json = JsonParser.parseString(new String(jar.getInputStream(entry).readAllBytes(), java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject();
        Set<String> result = new HashSet<>();
        for (var value : json.getAsJsonArray("values")) {
            String ref = value.isJsonPrimitive() ? value.getAsString() : value.getAsJsonObject().get("id").getAsString();
            if (ref.startsWith("#")) result.addAll(blockTag(jar, ref.substring(1), visiting));
            else result.add(Identifier.parse(ref).toString());
        }
        visiting.remove(name);
        return result;
    }
    static Tool bindRules(Tool source, ZipFile jar) throws Exception {
        List<Tool.Rule> rules = new ArrayList<>();
        for (var rule : source.rules()) {
            var key = rule.blocks().unwrapKey();
            if (key.isEmpty()) { rules.add(rule); continue; }
            List<Holder<Block>> blocks = new ArrayList<>();
            for (var name : blockTag(jar, key.get().location().toString(), new HashSet<>())) {
                var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(name));
                if (block == null) throw new IllegalStateException("missing block " + name);
                blocks.add(block.builtInRegistryHolder());
            }
            rules.add(new Tool.Rule(HolderSet.direct(blocks), rule.speed(), rule.correctForDrops()));
        }
        return new Tool(rules, source.defaultMiningSpeed(), source.damagePerBlock(), source.canDestroyBlocksInCreative());
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var world = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(world);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        Field unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        TestPlayer player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        var inventory = new Inventory(player, new EntityEquipment());
        var field = Player.class.getDeclaredField("inventory"); field.setAccessible(true); field.set(player, inventory);
        var applyDamage = ItemStack.class.getDeclaredMethod("applyDamage", int.class, net.minecraft.server.level.ServerPlayer.class, java.util.function.Consumer.class);
        applyDamage.setAccessible(true);
        player.submerged = new AttributeInstance(Attributes.SUBMERGED_MINING_SPEED, value -> {});
        player.submerged.setBaseValue(0.2);
        List<String> blocks = new ArrayList<>(List.of("grass_block", "dirt", "stone", "stone_bricks", "cobblestone", "oak_planks", "crafting_table"));
        blocks.addAll(Arrays.asList(args).subList(2, args.length));
        String[] materials = {"wooden", "stone", "copper", "iron", "golden", "diamond", "netherite"};
        String[] kinds = {"sword", "shovel", "pickaxe", "axe", "hoe"};
        JsonArray output = new JsonArray();
        try (var jar = new ZipFile(args[1])) {
            for (String material : materials) for (String kind : kinds) {
                String id = "minecraft:" + material + "_" + kind;
                var item = BuiltInRegistries.ITEM.getValue(Identifier.parse(id));
                if (item == null) throw new IllegalStateException("missing tool " + id);
                var stack = new ItemStack(item, 1);
                var tool = stack.get(DataComponents.TOOL);
                if (tool == null) throw new IllegalStateException("missing tool component " + id);
                stack.set(DataComponents.TOOL, bindRules(tool, jar));
                inventory.setItem(0, stack);
                JsonArray durability = new JsonArray();
                int maximum = stack.getMaxDamage();
                for (int initial : new int[]{0, maximum - 2, maximum - 1, maximum}) {
                    var damageStack = new ItemStack(item, 1);
                    damageStack.setDamageValue(initial);
                    applyDamage.invoke(damageStack, initial + tool.damagePerBlock(), null, (java.util.function.Consumer<ItemStack>)ignored -> {});
                    JsonArray result = new JsonArray();
                    result.add(initial); result.add(damageStack.getCount());
                    result.add(damageStack.isEmpty() ? -1 : damageStack.getDamageValue());
                    durability.add(result);
                }
                for (String block : blocks) {
                    var state = BuiltInRegistries.BLOCK.getValue(Identifier.withDefaultNamespace(block)).defaultBlockState();
                    for (boolean ground : new boolean[]{false, true}) for (boolean water : new boolean[]{false, true}) {
                        player.grounded = ground; player.underwater = water;
                        JsonArray row = new JsonArray();
                        row.add(id); row.add("minecraft:" + block); row.add(ground); row.add(water);
                        row.add(Float.floatToRawIntBits(stack.getDestroySpeed(state)));
                        row.add(player.hasCorrectToolForDrops(state));
                        row.add(Float.floatToRawIntBits(state.getDestroyProgress(player, null, BlockPos.ZERO)));
                        row.add(tool.damagePerBlock());
                        row.add(stack.getMaxDamage());
                        row.add(tool.canDestroyBlocksInCreative());
                        float progress = state.getDestroyProgress(player, null, BlockPos.ZERO);
                        float accumulated = 0.0f;
                        int ticks = 0;
                        JsonArray stages = new JsonArray();
                        while (accumulated < 1.0f && ticks < 10000) {
                            accumulated += progress; ticks++;
                            if (ground && !water) stages.add(accumulated < 1.0f ? (int)(accumulated * 10.0f) : -1);
                        }
                        row.add(ticks); row.add(stages);
                        row.add(durability);
                        output.add(row);
                    }
                }
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(output));
    }
}
"##;

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn falling_expiry_matches_java_entity_tick() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FallingExpiryOracle.java");
    let output = temp.path().join("expiry.json");
    std::fs::write(&source, include_str!("java/FallingExpiryOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    let java = java_bin();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FallingExpiryOracle")
        .arg(&output));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 62);
    for row in rows {
        if row.get("landing").is_some() {
            assert!(row["removed"].as_bool().unwrap(), "{row}");
            assert_eq!(row["dropped"].as_bool(), row["drops"].as_bool(), "{row}");
            continue;
        }
        let age = row["age"].as_i64().unwrap() as i32;
        let y = row["y"].as_i64().unwrap() as i32;
        let drops = row["drops"].as_bool().unwrap();
        let expires = native_session::falling_block::expires_in_air(age, y, -64, 320);
        assert_eq!(expires, row["removed"].as_bool().unwrap(), "{row}");
        assert_eq!(expires && drops, row["dropped"].as_bool().unwrap(), "{row}");
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn saved_falling_fields_match_java_entity() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SavedFallingOracle.java");
    std::fs::write(&source, include_str!("java/SavedFallingOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    let java = java_bin();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SavedFallingOracle")
        .arg(temp.path()));
    let rows: Vec<Value> =
        serde_json::from_slice(&std::fs::read(temp.path().join("expected.json")).unwrap()).unwrap();
    assert_eq!(rows.len(), 5);
    for (index, row) in rows.iter().enumerate() {
        let nbt = native_persistence::nbt::read(
            &std::fs::read(temp.path().join(format!("falling-{index}.nbt"))).unwrap(),
        )
        .unwrap();
        let saved = native_session::saved_falling_block::read(&nbt.value)
            .unwrap_or_else(|e| panic!("{e:#}; {:?}", nbt.value.get("BlockState")))
            .unwrap();
        assert_eq!(
            saved.time as i64,
            row["time"].as_i64().unwrap(),
            "case {index}"
        );
        assert_eq!(
            saved.drop_item,
            row["drop"].as_bool().unwrap(),
            "case {index}"
        );
        assert_eq!(
            saved.cancel_drop,
            row["cancel"].as_bool().unwrap(),
            "case {index}"
        );
        assert_eq!(saved.block.name, row["block"].as_str().unwrap());
        let properties: std::collections::BTreeMap<String, String> =
            serde_json::from_value(row["properties"].clone()).unwrap();
        assert_eq!(saved.block.properties, properties, "case {index}");
        assert_eq!(saved.position, glam::DVec3::new(1.25, 64.0, -2.5));
        assert_eq!(saved.uuid, Some([1, 2, 3, 4]));
        assert_eq!(*saved.source, nbt.value);
    }
    let mut original =
        native_persistence::nbt::read(&std::fs::read(temp.path().join("falling-0.nbt")).unwrap())
            .unwrap();
    let Tag::Compound(fields) = &mut original.value else {
        unreachable!()
    };
    for key in ["DropItem", "CancelDrop"] {
        fields
            .iter_mut()
            .find(|(name, _)| name.matches(key))
            .unwrap()
            .1 = Tag::Byte(0);
    }
    let draft = native_session::saved_falling_block::draft(
        &original,
        native_session::saved_falling_block::FallingSaveState {
            position: glam::DVec3::new(-1.5, 3.0, 2.5),
            velocity: glam::DVec3::new(0.2, -0.4, 0.0),
            on_ground: false,
            no_gravity: true,
            time: 101,
            drop_item: false,
            cancel_drop: true,
        },
    )
    .unwrap();
    let draft_file = temp.path().join("falling-draft.nbt");
    std::fs::write(
        &draft_file,
        native_persistence::nbt::write(draft.document()).unwrap(),
    )
    .unwrap();
    let chunk_state = native_session::saved_falling_block::FallingSaveState {
        position: glam::DVec3::new(3.5, 64.0, -2.5),
        velocity: glam::DVec3::new(0.2, -0.4, 0.0),
        on_ground: false,
        no_gravity: true,
        time: 101,
        drop_item: false,
        cancel_drop: true,
    };
    let replacement = native_session::saved_falling_block::draft(&original, chunk_state).unwrap();
    let item = native_persistence::nbt::NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![5, 6, 7, 8])),
            (
                "Pos".into(),
                Tag::List {
                    element_type: native_persistence::nbt::TagType::Double,
                    values: [4.5, 64.0, -2.5].into_iter().map(Tag::Double).collect(),
                },
            ),
            (
                "Item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:stone".into())),
                    ("count".into(), Tag::Int(3)),
                ]),
            ),
        ]),
    };
    let item_state = native_session::saved_item::read(&item.value)
        .unwrap()
        .unwrap();
    let item_replacement = native_session::saved_item::draft(
        &item,
        native_session::saved_item::ItemSaveState {
            position: glam::DVec3::new(5.5, 64.0, -2.5),
            velocity: glam::DVec3::ZERO,
            on_ground: false,
            no_gravity: true,
            age: 12,
            pickup_delay: 9,
            health: 5,
            fire_ticks: 0,
            invulnerable: false,
            invulnerable_time: 0,
            stack: &item_state.stack,
        },
    )
    .unwrap();
    let chunk = native_persistence::nbt::NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            ("Position".into(), Tag::IntArray(vec![0, -1])),
            ("unknown_chunk_field".into(), Tag::Long(19)),
            (
                "Entities".into(),
                Tag::List {
                    element_type: native_persistence::nbt::TagType::Compound,
                    values: vec![
                        original.value.clone(),
                        item.value.clone(),
                        Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:pig".into())),
                            ("UUID".into(), Tag::IntArray(vec![9, 10, 11, 12])),
                            ("unknown_pig_field".into(), Tag::Int(42)),
                        ]),
                    ],
                },
            ),
        ]),
    };
    let chunk_draft = native_session::saved_falling_block::draft_entity_chunk_with_items(
        &chunk,
        native_world::loaded::ChunkPos { x: 0, z: -1 },
        &[
            native_session::saved_falling_block::EntityReplacement::Falling(&replacement),
            native_session::saved_falling_block::EntityReplacement::Item(&item_replacement),
        ],
    )
    .unwrap();
    assert!(
        native_session::saved_falling_block::draft_entity_chunk_with_items(
            &chunk,
            native_world::loaded::ChunkPos { x: 0, z: -1 },
            &[
                native_session::saved_falling_block::EntityReplacement::Item(&item_replacement),
                native_session::saved_falling_block::EntityReplacement::Item(&item_replacement)
            ],
        )
        .is_err()
    );
    let chunk_file = temp.path().join("entity-chunk-draft.nbt");
    std::fs::write(
        &chunk_file,
        native_persistence::nbt::write(chunk_draft.document()).unwrap(),
    )
    .unwrap();
    let moved = native_session::saved_falling_block::draft(
        &original,
        native_session::saved_falling_block::FallingSaveState {
            position: glam::DVec3::new(16.5, 64.0, -2.5),
            ..chunk_state
        },
    )
    .unwrap();
    let source_pos = native_world::loaded::ChunkPos { x: 0, z: -1 };
    let target_pos = native_world::loaded::ChunkPos { x: 1, z: -1 };
    let originals =
        std::collections::BTreeMap::from([(source_pos, Some(&chunk)), (target_pos, None)]);
    let moved_drafts =
        native_session::saved_falling_block::draft_entity_chunks_with_optional_sources(
            &originals,
            &[native_session::saved_falling_block::EntityReplacement::Falling(&moved)],
        )
        .unwrap();
    assert!(moved_drafts[&target_pos].source_was_absent());
    let moved_file = temp.path().join("entity-chunk-moved.nbt");
    std::fs::write(
        &moved_file,
        native_persistence::nbt::write(moved_drafts[&target_pos].document()).unwrap(),
    )
    .unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SavedFallingOracle")
        .arg(temp.path())
        .arg(draft_file)
        .arg(chunk_file)
        .arg(moved_file));
}

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
        "mining oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ore_mining_and_harvest_tiers_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("ToolMiningOracle.java");
    let result = temp.path().join("ore.json");
    std::fs::write(&source, TOOL_ORACLE).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let ores = [
        "coal_ore",
        "deepslate_coal_ore",
        "iron_ore",
        "deepslate_iron_ore",
        "copper_ore",
        "deepslate_copper_ore",
        "gold_ore",
        "deepslate_gold_ore",
        "diamond_ore",
        "deepslate_diamond_ore",
        "emerald_ore",
        "deepslate_emerald_ore",
        "lapis_ore",
        "deepslate_lapis_ore",
        "redstone_ore",
        "deepslate_redstone_ore",
        "nether_gold_ore",
        "nether_quartz_ore",
    ];
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("ToolMiningOracle")
        .arg(&result)
        .arg(game.join("versions/26.3/26.3.jar"))
        .args(ores));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
    let mut checked = 0;
    for row in rows {
        let name = row[1].as_str().unwrap();
        if !ores.iter().any(|ore| name == format!("minecraft:{ore}")) {
            continue;
        }
        let material = Material::from_block_name(name).unwrap();
        let item =
            ItemStack::new(row[0].as_str().unwrap().into(), 1, 1, Tag::Compound(vec![])).unwrap();
        let properties = mining_properties(material, Some(&item)).unwrap();
        assert_eq!(
            properties.speed.to_bits(),
            row[4].as_i64().unwrap() as i32 as u32,
            "{row}"
        );
        assert_eq!(
            properties.correct_for_drops,
            row[5].as_bool().unwrap(),
            "{row}"
        );
        assert_eq!(
            tool_progress(
                material,
                Some(&item),
                row[2].as_bool().unwrap(),
                row[3].as_bool().unwrap()
            )
            .unwrap()
            .to_bits(),
            row[6].as_i64().unwrap() as i32 as u32,
            "{row}"
        );
        checked += 1;
    }
    assert_eq!(checked, ores.len() * 7 * 5 * 4);
    eprintln!("Compared {checked} ore/tool cases with Minecraft 26.3");
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ordinary_tool_mining_matches_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let loot =
        std::sync::Arc::new(native_resources::loot::BlockLootTables::load(&mut resources).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("ToolMiningOracle.java");
    let result = temp.path().join("result.json");
    std::fs::write(&source, TOOL_ORACLE).unwrap();
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
        .arg("ToolMiningOracle")
        .arg(&result)
        .arg(game.join("versions/26.3/26.3.jar"))
        .args(
            native_core::wood::Wood::all()
                .map(|wood| wood.id().strip_prefix("minecraft:").unwrap()),
        )
        .args([
            "coarse_dirt",
            "rooted_dirt",
            "podzol",
            "mycelium",
            "dirt_path",
            "farmland",
            "sand",
            "red_sand",
            "gravel",
            "stone_button",
        ]));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
    assert_eq!(
        rows.len(),
        7 * 5 * (17 + native_core::wood::Wood::all().count()) * 4
    );
    for row in &rows {
        let id = row[0].as_str().unwrap();
        let material = Material::from_block_name(row[1].as_str().unwrap()).unwrap();
        let tool = ItemStack::new(id.into(), 1, 1, Tag::Compound(vec![])).unwrap();
        let properties = mining_properties(material, Some(&tool)).unwrap();
        assert_eq!(
            properties.speed.to_bits(),
            row[4].as_i64().unwrap() as i32 as u32,
            "{row}"
        );
        assert_eq!(
            properties.correct_for_drops,
            row[5].as_bool().unwrap(),
            "{row}"
        );
        assert_eq!(
            tool_progress(
                material,
                Some(&tool),
                row[2].as_bool().unwrap(),
                row[3].as_bool().unwrap()
            )
            .unwrap()
            .to_bits(),
            row[6].as_i64().unwrap() as i32 as u32,
            "{row}"
        );
        assert_eq!(
            u64::from(properties.damage_per_block),
            row[7].as_u64().unwrap(),
            "{row}"
        );
        assert_eq!(
            u64::from(native_session::inventory::ordinary_item_default_durability(id).unwrap()),
            row[8].as_u64().unwrap(),
            "{row}"
        );
        assert_eq!(
            properties.can_destroy_in_creative,
            row[9].as_bool().unwrap(),
            "{row}"
        );
        if row[2].as_bool().unwrap()
            && !row[3].as_bool().unwrap()
            && row[1].as_str().unwrap() != "minecraft:gravel"
            && row[1].as_str().unwrap() != "minecraft:stone_button"
        {
            let block = Block::from_item_id(row[1].as_str().unwrap()).unwrap();
            let target = glam::IVec3::new(0, 4, 3);
            let mut world = World::default();
            world.set_block(glam::IVec3::new(0, 2, 5), Block::Stone);
            world.set_block(target, block);
            let mut session = LocalSession::new(world);
            session.set_block_loot(std::sync::Arc::clone(&loot));
            let mut inventory = PlayerInventory::default();
            let custom = Tag::Compound(vec![("sample".into(), Tag::Int(42))]);
            let tool = ItemStack::new(
                id.into(),
                1,
                1,
                Tag::Compound(vec![("minecraft:custom_data".into(), custom.clone())]),
            );
            inventory.set_slot(PlayerSlot::Storage(0), tool);
            session.set_player_inventory(inventory);
            session.update(std::time::Duration::from_millis(150));
            assert!(session.snapshot().player.on_ground);
            session.submit_command(PlayerCommand::Break);
            let ticks = row[10].as_u64().unwrap();
            for tick in 1..=ticks {
                session.update(std::time::Duration::from_millis(50));
                let snapshot = session.snapshot();
                assert_eq!(
                    snapshot.world.demo().unwrap().block(target),
                    if tick == ticks { Block::Air } else { block },
                    "break tick {tick}, {row}"
                );
                let stage = snapshot
                    .breaking
                    .map(|progress| i64::from(progress.stage))
                    .unwrap_or(-1);
                assert_eq!(
                    stage,
                    row[11][tick as usize - 1].as_i64().unwrap(),
                    "stage at tick {tick}, {row}"
                );
            }
            let snapshot = session.snapshot();
            let hand = snapshot.inventory.slot(PlayerSlot::Storage(0)).unwrap();
            assert_eq!(
                hand.components()
                    .get("minecraft:damage")
                    .and_then(Tag::as_java_int),
                Some(properties.damage_per_block as i32),
                "{row}"
            );
            assert_eq!(
                hand.components().get("minecraft:custom_data"),
                Some(&custom)
            );
            if let Block::Wood(wood, _) = block {
                assert_eq!(snapshot.dropped_items.len(), 1, "{row}");
                assert_eq!(snapshot.dropped_items[0].stack.id(), wood.id());
                assert_eq!(snapshot.dropped_items[0].stack.max_count(), Some(64));
            }
            if matches!(block, Block::Stone | Block::Cobblestone) {
                if properties.correct_for_drops {
                    assert_eq!(snapshot.dropped_items.len(), 1, "{row}");
                    assert_eq!(
                        snapshot.dropped_items[0].stack.id(),
                        "minecraft:cobblestone"
                    );
                    assert_eq!(snapshot.dropped_items[0].stack.max_count(), Some(64));
                } else {
                    assert!(snapshot.dropped_items.is_empty(), "{row}");
                }
                if block != Block::Stone {
                    continue;
                }
                for expected in row[12].as_array().unwrap() {
                    let mut world = World::default();
                    world.set_block(glam::IVec3::new(0, 2, 5), Block::Stone);
                    world.set_block(target, Block::Stone);
                    let mut session = LocalSession::new(world);
                    let mut inventory = PlayerInventory::default();
                    inventory.set_slot(
                        PlayerSlot::Storage(0),
                        ItemStack::new(
                            id.into(),
                            1,
                            1,
                            Tag::Compound(vec![(
                                "minecraft:damage".into(),
                                Tag::Int(expected[0].as_i64().unwrap() as i32),
                            )]),
                        ),
                    );
                    session.set_player_inventory(inventory);
                    session.update(std::time::Duration::from_millis(150));
                    session.submit_command(PlayerCommand::Break);
                    for _ in 0..ticks {
                        session.update(std::time::Duration::from_millis(50));
                    }
                    let after = session.snapshot();
                    let hand = after.inventory.slot(PlayerSlot::Storage(0));
                    assert_eq!(
                        hand.map_or(0, |item| item.count()),
                        expected[1].as_u64().unwrap() as u32,
                        "durability: {id}, {expected}"
                    );
                    assert_eq!(
                        hand.and_then(|item| item.components().get("minecraft:damage"))
                            .and_then(Tag::as_java_int)
                            .unwrap_or(-1),
                        expected[2].as_i64().unwrap() as i32,
                        "durability: {id}, {expected}"
                    );
                }
            }
        }
    }
    eprintln!(
        "Compared {} ordinary tool mining cases with Java 26.3",
        rows.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn basic_block_destroy_progress_matches_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("MiningOracle.java");
    let result = temp.path().join("result.json");
    std::fs::write(&source, ORACLE).unwrap();
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
        .arg("MiningOracle")
        .arg(&result));
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
    let mut cases = vec![
        (Material::Grass, Block::Grass),
        (Material::Dirt, Block::Dirt),
        (Material::Stone, Block::Stone),
        (Material::StoneBricks, Block::StoneBricks),
    ];
    cases
        .extend(native_core::wood::Planks::ALL.map(|kind| (Material::Planks, Block::Planks(kind))));
    cases.push((Material::CraftingTable, Block::CraftingTable));
    cases.extend([
        (Material::Dirt, Block::CoarseDirt),
        (Material::Dirt, Block::RootedDirt),
        (Material::Dirt, Block::Podzol),
        (Material::Grass, Block::Mycelium),
        (Material::DirtPath, Block::DirtPath),
        (Material::Grass, Block::Farmland(0)),
    ]);
    cases.push((Material::StoneButton, Block::Air));
    assert_eq!(rows.len(), cases.len() * 12);
    for row in &rows {
        let (material, block) = cases[row[0].as_u64().unwrap() as usize];
        assert_eq!(
            material.hardness().to_bits(),
            row[4].as_i64().unwrap() as i32 as u32,
            "hardness: {row}"
        );
        assert_eq!(
            material.needs_tool(),
            row[5].as_bool().unwrap(),
            "tool requirement: {row}"
        );
        let progress = basic_progress(
            material,
            row[1].as_bool().unwrap(),
            row[2].as_bool().unwrap(),
        );
        assert_eq!(
            progress.to_bits(),
            row[6].as_i64().unwrap() as i32 as u32,
            "progress: {row}"
        );
        if material != Material::StoneButton
            && row[1].as_bool().unwrap()
            && !row[2].as_bool().unwrap()
        {
            let mut world = World::default();
            world.set_block(glam::IVec3::new(0, 2, 5), Block::Stone);
            let target = glam::IVec3::new(0, 4, 3);
            world.set_block(target, block);
            let mut session = LocalSession::new(world);
            let mut inventory = PlayerInventory::default();
            let hand = row[3].as_u64().unwrap();
            if hand != 0 {
                let (max, components) = if hand == 2 {
                    (
                        99,
                        Tag::Compound(vec![("minecraft:max_stack_size".into(), Tag::Int(99))]),
                    )
                } else {
                    (64, Tag::Compound(vec![]))
                };
                inventory.set_slot(
                    PlayerSlot::Storage(0),
                    ItemStack::new("minecraft:stone".into(), 1, max, components),
                );
            }
            session.set_player_inventory(inventory);
            session.update(std::time::Duration::from_millis(150));
            assert!(session.snapshot().player.on_ground);
            session.submit_command(PlayerCommand::Break);
            session.update(std::time::Duration::from_millis(50));
            assert_eq!(
                session.snapshot().world.demo().unwrap().block(target),
                block
            );
            assert_eq!(
                i64::from(session.snapshot().breaking.unwrap().stage),
                row[8][0].as_i64().unwrap(),
                "first-tick stage, row {row}"
            );
            let expected_ticks = row[7].as_u64().unwrap();
            for tick in 2..=expected_ticks {
                session.update(std::time::Duration::from_millis(50));
                let actual = session.snapshot().world.demo().unwrap().block(target);
                assert_eq!(
                    actual,
                    if tick == expected_ticks {
                        Block::Air
                    } else {
                        block
                    },
                    "tick {tick}, row {row}"
                );
                let stage = session
                    .snapshot()
                    .breaking
                    .map(|progress| i32::from(progress.stage))
                    .unwrap_or(-1);
                assert_eq!(
                    stage,
                    row[8][tick as usize - 1].as_i64().unwrap() as i32,
                    "breaking stage at tick {tick}, row {row}"
                );
            }
        }
    }
    println!(
        "Compared {} block, hand, ground and water cases with Java 26.3",
        rows.len()
    );
}
