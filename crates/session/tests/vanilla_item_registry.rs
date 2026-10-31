//! Compare implemented item semantics with the installed 26.3 registry.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_session::inventory::{ordinary_item_default_durability, ordinary_item_default_max};
use serde_json::Value;

const ORACLE: &str = r#"
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.component.LodestoneTracker;
import net.minecraft.world.item.enchantment.Enchantments;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.block.RotatedPillarBlock;
import net.minecraft.core.Direction;
import net.minecraft.world.entity.player.Player;

public class ItemRegistryOracle {
  static class PlacementContext extends BlockPlaceContext {
    Direction face;
    PlacementContext() { super((Player)null,null,ItemStack.EMPTY,null); }
    @Override public Direction getClickedFace() { return face; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var world=VanillaRegistries.createWorldLookup();
    var registries=VanillaRegistries.createReloadableLookup(world);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)unsafeField.get(null);
    var context=(PlacementContext)unsafe.allocateInstance(PlacementContext.class);
    var supportedWood=new HashSet<>(Arrays.asList(args).subList(1,args.length));
    var mending=world.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(Enchantments.MENDING);
    var result=new JsonObject();
    for(var item:BuiltInRegistries.ITEM) {
      var v=new JsonObject();
      v.addProperty("max",item.getDefaultMaxStackSize());
      v.addProperty("durability",item.components().getOrDefault(DataComponents.MAX_DAMAGE,0));
      v.addProperty("special_click",Arrays.stream(item.getClass().getMethods()).anyMatch(m ->
        (m.getName().equals("overrideStackedOnOther") || m.getName().equals("overrideOtherStackedOnMe"))
        && m.getDeclaringClass()!=Item.class));
      v.addProperty("equippable",item.components().has(DataComponents.EQUIPPABLE));
      v.addProperty("tool",item.components().has(DataComponents.TOOL));
      v.addProperty("passive_use", item.getClass().getMethod("useOn",net.minecraft.world.item.context.UseOnContext.class).getDeclaringClass()==Item.class
        && item.getClass().getMethod("use",net.minecraft.world.level.Level.class,Player.class,net.minecraft.world.InteractionHand.class).getDeclaringClass()==Item.class
        && !item.components().has(DataComponents.BLOCK_TRANSFORMER)
        && !item.components().has(DataComponents.CONSUMABLE)
        && !item.components().has(DataComponents.EQUIPPABLE)
        && !item.components().has(DataComponents.BLOCKS_ATTACKS)
        && !item.components().has(DataComponents.KINETIC_WEAPON));
      v.addProperty("transformer_only_use", item.getClass().getMethod("useOn",net.minecraft.world.item.context.UseOnContext.class).getDeclaringClass()==Item.class
        && item.getClass().getMethod("use",net.minecraft.world.level.Level.class,Player.class,net.minecraft.world.InteractionHand.class).getDeclaringClass()==Item.class
        && item.components().has(DataComponents.BLOCK_TRANSFORMER)
        && !item.components().has(DataComponents.CONSUMABLE)
        && !item.components().has(DataComponents.EQUIPPABLE)
        && !item.components().has(DataComponents.BLOCKS_ATTACKS)
        && !item.components().has(DataComponents.KINETIC_WEAPON));
      v.addProperty("remainder",item.getCraftingRemainder()!=null);
      String id=BuiltInRegistries.ITEM.getKey(item).toString();
      var initial=new ItemStack(item,1);
      if(!initial.isEmpty()) {
        var foil=new JsonArray();
        foil.add(initial.hasFoil());
        var forced=initial.copy(); forced.set(DataComponents.ENCHANTMENT_GLINT_OVERRIDE,true); foil.add(forced.hasFoil());
        forced=initial.copy(); forced.set(DataComponents.ENCHANTMENT_GLINT_OVERRIDE,false); foil.add(forced.hasFoil());
        var removed=initial.copy(); removed.remove(DataComponents.ENCHANTMENT_GLINT_OVERRIDE); foil.add(removed.hasFoil());
        var enchanted=initial.copy(); enchanted.enchant(mending,1); foil.add(enchanted.hasFoil());
        var tracked=initial.copy(); tracked.set(DataComponents.LODESTONE_TRACKER,new LodestoneTracker(Optional.empty(),false)); foil.add(tracked.hasFoil());
        v.add("foil",foil);
      }
      if(item instanceof BlockItem blockItem) {
        var block=blockItem.getBlock();
        v.addProperty("block",BuiltInRegistries.BLOCK.getKey(block).toString());
        var properties=new JsonObject();
        block.defaultBlockState().getValues().forEach(value -> properties.addProperty(value.property().getName(),value.valueName()));
        v.add("block_properties",properties);
        if (supportedWood.contains(id) && block instanceof RotatedPillarBlock pillar) {
          var axes=new JsonObject();
          for(var face:Direction.values()) {
            context.face=face;
            axes.addProperty(face.getName(), pillar.getStateForPlacement(context).getValue(RotatedPillarBlock.AXIS).getName());
          }
          v.add("placement_axes",axes);
        }
      }
      if(id.matches("minecraft:(wooden|stone|copper|iron|golden|diamond|netherite)_sword")) {
        var bars=new JsonArray();
        int maximum=item.components().get(DataComponents.MAX_DAMAGE);
        for(int damage=0;damage<=maximum+1;damage++) {
          var stack=new ItemStack(item,1); stack.set(DataComponents.DAMAGE,damage);
          if(!stack.isBarVisible()) bars.add(JsonNull.INSTANCE);
          else { var bar=new JsonArray(); bar.add(stack.getBarWidth()); bar.add(stack.getBarColor() & 0xffffff); bars.add(bar); }
        }
        v.add("bars",bars);
      }
      result.add(id,v);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(result));
  }
}
"#;

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
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "item registry oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn implemented_item_defaults_and_click_assumptions_match_vanilla() {
    use native_persistence::nbt::Tag;
    use native_session::inventory::ItemStack;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("ItemRegistryOracle.java");
    let output = temp.path().join("items.json");
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
        .arg("ItemRegistryOracle")
        .arg(&output)
        .args(native_core::wood::Wood::all().map(|wood| wood.id())));
    let result: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(result["minecraft:stick"]["passive_use"], true);
    for material in [
        "wooden",
        "stone",
        "copper",
        "iron",
        "golden",
        "diamond",
        "netherite",
    ] {
        for kind in ["pickaxe", "sword"] {
            let id = format!("minecraft:{material}_{kind}");
            assert_eq!(result[&id]["passive_use"], true, "{id}");
        }
        for kind in ["axe", "shovel", "hoe"] {
            let id = format!("minecraft:{material}_{kind}");
            assert_eq!(result[&id]["transformer_only_use"], true, "{id}");
        }
    }
    let mut checked = 0;
    let mut bars_checked = 0;
    for (id, value) in result.as_object().unwrap() {
        if let Some(expected) = value.get("foil").and_then(Value::as_array) {
            let patches = [
                vec![],
                vec![("minecraft:enchantment_glint_override".into(), Tag::Byte(1))],
                vec![("minecraft:enchantment_glint_override".into(), Tag::Byte(0))],
                vec![(
                    "!minecraft:enchantment_glint_override".into(),
                    Tag::Compound(vec![]),
                )],
                vec![(
                    "minecraft:enchantments".into(),
                    Tag::Compound(vec![("minecraft:mending".into(), Tag::Int(1))]),
                )],
                vec![("minecraft:lodestone_tracker".into(), Tag::Compound(vec![]))],
            ];
            for (index, patch) in patches.into_iter().enumerate() {
                let item = ItemStack::unresolved(id.clone(), 1, Tag::Compound(patch)).unwrap();
                assert_eq!(
                    item.has_glint(),
                    expected[index].as_bool().unwrap(),
                    "{id} foil case {index}"
                );
            }
        }
        let Some(max) = ordinary_item_default_max(id) else {
            continue;
        };
        assert_eq!(Some(u64::from(max)), value["max"].as_u64(), "{id}");
        assert_eq!(
            ordinary_item_default_durability(id).unwrap_or(0) as u64,
            value["durability"].as_u64().unwrap(),
            "{id}: durability"
        );
        assert_eq!(value["special_click"], false, "{id}: special click");
        assert_eq!(value["equippable"], false, "{id}: equippable");
        assert_eq!(value["remainder"], false, "{id}: crafting remainder");
        if max == 64 {
            assert_eq!(value["tool"], false, "{id}: tool component");
        }
        if let Some(bars) = value.get("bars").and_then(Value::as_array) {
            for (damage, expected) in bars.iter().enumerate() {
                let item = ItemStack::new(
                    id.clone(),
                    1,
                    1,
                    Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(damage as i32))]),
                )
                .unwrap();
                let actual = item
                    .durability_bar()
                    .map(|(width, color)| {
                        serde_json::json!([
                            width,
                            ((color[0] as u32) << 16) | ((color[1] as u32) << 8) | color[2] as u32
                        ])
                    })
                    .unwrap_or(Value::Null);
                assert_eq!(&actual, expected, "{id} damage {damage}");
                bars_checked += 1;
            }
        }
        checked += 1;
    }
    assert!(checked >= 150, "only {checked} implemented IDs were found");
    // These family expansions must all exist; a typo must never become a made-up item.
    for material in [
        "wooden",
        "stone",
        "copper",
        "iron",
        "golden",
        "diamond",
        "netherite",
    ] {
        for kind in ["sword", "shovel", "pickaxe", "axe", "hoe"] {
            let id = format!("minecraft:{material}_{kind}");
            assert_eq!(ordinary_item_default_max(&id), Some(1));
            assert!(result.get(&id).is_some(), "missing {id}");
        }
    }
    for wood in [
        "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak",
        "poplar",
    ] {
        for suffix in ["planks", "log", "wood"] {
            let id = format!("minecraft:{wood}_{suffix}");
            assert_eq!(ordinary_item_default_max(&id), Some(64));
            assert!(result.get(&id).is_some(), "missing {id}");
            if suffix != "planks" {
                let id = format!("minecraft:stripped_{wood}_{suffix}");
                assert_eq!(ordinary_item_default_max(&id), Some(64));
                assert!(result.get(&id).is_some(), "missing {id}");
            }
        }
    }
    for id in native_core::wood::Planks::ALL
        .into_iter()
        .map(|planks| planks.id())
        .chain(["minecraft:crafting_table", "minecraft:cobblestone"])
    {
        let block = native_world::Block::from_item_id(id).unwrap();
        assert_eq!(block.item_id(), Some(id));
        assert_eq!(result[id]["block"].as_str(), Some(id), "{id} block item");
        assert_eq!(
            result[id]["block_properties"].as_object().unwrap().len(),
            0,
            "{id} default block state"
        );
    }
    let faces = [
        ("east", glam::IVec3::X),
        ("west", glam::IVec3::NEG_X),
        ("up", glam::IVec3::Y),
        ("down", glam::IVec3::NEG_Y),
        ("south", glam::IVec3::Z),
        ("north", glam::IVec3::NEG_Z),
    ];
    for wood in native_core::wood::Wood::all() {
        let id = wood.id();
        assert_eq!(ordinary_item_default_max(id), Some(64), "{id}");
        assert_eq!(result[id]["block"].as_str(), Some(id), "{id}");
        assert_eq!(
            result[id]["block_properties"]["axis"].as_str(),
            Some("y"),
            "{id}"
        );
        let block = native_world::Block::from_item_id(id).unwrap();
        for (face_name, direction) in faces {
            let native_world::Block::Wood(actual, axis) = block.orient_for_face(direction) else {
                panic!("not wood: {id}")
            };
            assert_eq!(actual, wood);
            assert_eq!(
                result[id]["placement_axes"][face_name].as_str(),
                Some(axis.name()),
                "{id}, {face_name}"
            );
        }
    }
    eprintln!("verified {checked} implemented item defaults and {bars_checked} durability bars");
}

#[test]
#[ignore = "requires locally installed Minecraft 26.3 assets"]
fn ordinary_wood_recipes_consume_inputs_and_create_vanilla_results() {
    use native_persistence::nbt::Tag;
    use native_resources::{packs::ResourceStack, recipes::CraftingRecipes};
    use native_session::inventory::{ItemStack, PlayerInventory, PlayerSlot};

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = ResourceStack::open(&game).unwrap();
    let recipes = CraftingRecipes::load(&mut resources).unwrap();
    for (ingredient, slots, output, count) in [
        ("minecraft:oak_log", &[0][..], "minecraft:oak_planks", 4),
        ("minecraft:oak_planks", &[0, 2][..], "minecraft:stick", 4),
        (
            "minecraft:oak_planks",
            &[0, 1, 2, 3][..],
            "minecraft:crafting_table",
            1,
        ),
    ] {
        let mut inventory = PlayerInventory::default();
        assert!(inventory.toggle());
        for &slot in slots {
            let stack = ItemStack::new(ingredient.into(), 1, 64, Tag::Compound(vec![])).unwrap();
            assert!(inventory.set_slot(PlayerSlot::Crafting(slot), Some(stack)));
        }
        inventory.refresh_crafting(&recipes);
        let result = inventory.slot(PlayerSlot::CraftResult).unwrap();
        assert_eq!((result.id(), result.count()), (output, count));
        assert!(inventory.take_crafting_result(&recipes));
        assert_eq!(
            (
                inventory.cursor().unwrap().id(),
                inventory.cursor().unwrap().count()
            ),
            (output, count)
        );
        assert!(
            slots
                .iter()
                .all(|&slot| inventory.slot(PlayerSlot::Crafting(slot)).is_none())
        );
        assert!(inventory.toggle());
        assert_eq!(inventory.slot(PlayerSlot::Storage(0)).unwrap().id(), output);
    }
}
