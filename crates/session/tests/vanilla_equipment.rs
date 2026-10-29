//! Compare player armor-slot interactions with the installed Java 26.3 menu.

use native_persistence::nbt::Tag;
use native_resources::{enchantments::OxygenEnchantments, packs::ResourceStack};
use native_session::{
    equippable::saved_inventory_max,
    inventory::{ClickButton, ItemStack, PlayerInventory, PlayerSlot},
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

const ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.Holder;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.*;
import net.minecraft.resources.Identifier;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.player.*;
import net.minecraft.world.flag.*;
import net.minecraft.world.inventory.*;
import net.minecraft.world.item.*;
import net.minecraft.world.item.enchantment.Enchantments;
import net.minecraft.world.item.equipment.Equippable;
import net.minecraft.world.level.*;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.util.Prediction;

public class EquipmentOracle {
  static class TestLevel extends ClientLevel {
    TestLevel() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.VANILLA_SET; }
  }
  static class TestPlayer extends Player {
    Inventory inventory; Abilities abilities; Level world; boolean creative; ItemStack dropped;
    TestPlayer() { super(null,null); }
    @Override public GameType gameMode() { return creative ? GameType.CREATIVE : GameType.SURVIVAL; }
    @Override public Level level() { return world; }
    @Override public Inventory getInventory() { return inventory; }
    @Override public boolean hasInfiniteMaterials() { return creative; }
    @Override public Holder<EntityType<?>> typeHolder() { return EntityTypes.PLAYER.builtInRegistryHolder(); }
    @Override public void onEquipItem(EquipmentSlot slot, ItemStack old, ItemStack now) { }
    @Override public ItemEntity drop(ItemStack stack, boolean trace, Prediction prediction) { dropped=stack.copy(); return null; }
  }
  static net.minecraft.core.HolderLookup.Provider world;
  static ItemStack stack(JsonElement value) {
    if (value == null || value.isJsonNull()) return ItemStack.EMPTY;
    var v=value.getAsJsonObject();
    var item=BuiltInRegistries.ITEM.getValue(Identifier.parse(v.get("id").getAsString()));
    var result=new ItemStack(item,v.get("count").getAsInt());
    if (v.has("enchantment")) {
      var key=v.get("enchantment").getAsString().equals("binding") ? Enchantments.BINDING_CURSE : Enchantments.RESPIRATION;
      result.enchant(world.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(key), v.get("level").getAsInt());
    }
    if (v.has("custom")) {
      var builder=Equippable.builder(EquipmentSlot.HEAD);
      if (v.get("custom").getAsString().equals("zombie")) builder.setAllowedEntities(EntityTypes.ZOMBIE);
      result.set(DataComponents.EQUIPPABLE,builder.build());
    }
    return result;
  }
  static JsonElement output(ItemStack item) {
    if(item.isEmpty()) return JsonNull.INSTANCE;
    var result=new JsonArray(); result.add(BuiltInRegistries.ITEM.getKey(item.getItem()).toString()); result.add(item.getCount()); return result;
  }
  static int index(int source) { return source>=100 ? 5+(source-100) : source==40 ? 45 : source<9 ? 36+source : source; }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    world=VanillaRegistries.createWorldLookup(); var registry=VanillaRegistries.createReloadableLookup(world);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registry)) pending.apply();
    Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
    player.world=(TestLevel)unsafe.allocateInstance(TestLevel.class); player.abilities=new Abilities();
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray(); var result=new JsonArray();
    for(var value:cases) {
      var c=value.getAsJsonObject(); player.creative=c.get("creative").getAsBoolean();
      player.inventory=new Inventory(player,new EntityEquipment()); player.dropped=ItemStack.EMPTY;
      var slots=c.getAsJsonArray("storage"); for(int i=0;i<36;i++) player.inventory.setItem(i,stack(slots.get(i)));
      var armor=c.getAsJsonArray("armor"); for(int i=0;i<4;i++) player.inventory.setItem(39-i,stack(armor.get(i)));
      player.inventory.setItem(40,stack(c.get("offhand")));
      var menu=new InventoryMenu(player.inventory,false,player); menu.setCarried(stack(c.get("cursor")));
      int source=index(c.get("source").getAsInt()), button=c.get("button").getAsInt();
      var operation=c.get("operation").getAsString();
      switch(operation) {
        case "click" -> menu.clicked(source,button,ContainerInput.PICKUP,player);
        case "quick" -> menu.clicked(source,button,ContainerInput.QUICK_MOVE,player);
        case "swap" -> menu.clicked(source,button,ContainerInput.SWAP,player);
        case "throw" -> menu.clicked(source,button,ContainerInput.THROW,player);
        default -> throw new IllegalArgumentException(operation);
      }
      var row=new JsonArray(); var resultStorage=new JsonArray(); var resultArmor=new JsonArray();
      for(int i=0;i<36;i++) resultStorage.add(output(player.inventory.getItem(i)));
      for(int i=0;i<4;i++) resultArmor.add(output(player.inventory.getItem(39-i)));
      row.add(resultStorage); row.add(resultArmor); row.add(output(player.inventory.getItem(40))); row.add(output(menu.getCarried())); row.add(output(player.dropped)); result.add(row);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(result));
    var prototypes=new JsonObject();
    for(var item:BuiltInRegistries.ITEM) {
      var equippable=item.components().get(DataComponents.EQUIPPABLE);
      if(equippable==null || !equippable.canBeEquippedBy(EntityTypes.PLAYER.builtInRegistryHolder())) continue;
      var value=new JsonArray(); value.add(equippable.slot().getName()); value.add(item.getDefaultMaxStackSize());
      value.add(item.components().getOrDefault(DataComponents.MAX_DAMAGE,0));
      value.add(Arrays.stream(item.getClass().getMethods()).anyMatch(m ->
        (m.getName().equals("overrideStackedOnOther") || m.getName().equals("overrideOtherStackedOnMe")) && m.getDeclaringClass()!=Item.class));
      prototypes.add(BuiltInRegistries.ITEM.getKey(item).toString(),value);
    }
    Files.writeString(Path.of(args[2]),new Gson().toJson(prototypes));
  }
}
"#;

fn java_bin() -> PathBuf {
    std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin")
}
fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let metadata: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in metadata["libraries"].as_array().unwrap() {
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
        "equipment oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn item(id: &str, count: u32) -> Value {
    json!({"id":format!("minecraft:{id}"),"count":count})
}
fn enchanted(id: &str, enchantment: &str) -> Value {
    json!({"id":format!("minecraft:{id}"),"count":1,"enchantment":enchantment,"level":1})
}
fn new_case(operation: &str, source: i32, button: i32, creative: bool) -> Value {
    json!({"operation":operation,"source":source,"button":button,"creative":creative,"storage":vec![Value::Null;36],"armor":vec![Value::Null;4],"offhand":null,"cursor":null})
}
fn native_item(value: &Value) -> Option<ItemStack> {
    if value.is_null() {
        return None;
    }
    let id = value["id"].as_str().unwrap().to_owned();
    let mut fields = Vec::new();
    if let Some(enchantment) = value.get("enchantment").and_then(Value::as_str) {
        let name = if enchantment == "binding" {
            "minecraft:binding_curse"
        } else {
            "minecraft:respiration"
        };
        fields.push((
            "minecraft:enchantments".into(),
            Tag::Compound(vec![(
                name.into(),
                Tag::Int(value["level"].as_i64().unwrap() as i32),
            )]),
        ));
    }
    if let Some(custom) = value.get("custom").and_then(Value::as_str) {
        let mut component = vec![("slot".into(), Tag::String("head".into()))];
        if custom == "zombie" {
            component.push((
                "allowed_entities".into(),
                Tag::String("minecraft:zombie".into()),
            ));
        }
        fields.push(("minecraft:equippable".into(), Tag::Compound(component)));
    }
    let components = Tag::Compound(fields);
    let max = saved_inventory_max(&id, &components).expect("supported test item");
    ItemStack::new(id, value["count"].as_u64().unwrap() as u32, max, components)
}
fn output(item: Option<&ItemStack>) -> Value {
    item.map_or(Value::Null, |item| json!([item.id(), item.count()]))
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn equipment_slot_interactions_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = ResourceStack::open(&game).unwrap();
    let enchantments = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
    let samples = vec![
        Value::Null,
        item("iron_helmet", 1),
        item("diamond_helmet", 1),
        item("iron_boots", 1),
        item("carved_pumpkin", 64),
        item("stone", 64),
        enchanted("iron_helmet", "respiration"),
        enchanted("iron_helmet", "binding"),
        json!({"id":"minecraft:stone","count":32,"custom":"player"}),
        json!({"id":"minecraft:stone","count":32,"custom":"zombie"}),
    ];
    let mut cases = Vec::new();
    for creative in [false, true] {
        for slot in [0, 3] {
            for existing in &samples {
                for carried in &samples {
                    for button in [0, 1] {
                        let mut c = new_case("click", 100 + slot, button, creative);
                        c["armor"][slot as usize] = existing.clone();
                        c["cursor"] = carried.clone();
                        cases.push(c);
                    }
                }
            }
        }
        for item in &samples {
            for source in [0, 9, 100, 103] {
                let mut c = new_case("quick", source, 0, creative);
                if source >= 100 {
                    c["armor"][(source - 100) as usize] = item.clone();
                } else {
                    c["storage"][source as usize] = item.clone();
                }
                cases.push(c);
            }
            for existing in &samples {
                let mut c = new_case("swap", 100, 0, creative);
                c["armor"][0] = existing.clone();
                c["storage"][0] = item.clone();
                cases.push(c);
            }
            for button in [0, 1] {
                let mut c = new_case("throw", 100, button, creative);
                c["armor"][0] = item.clone();
                cases.push(c);
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("EquipmentOracle.java");
    let input = temp.path().join("cases.json");
    let output_file = temp.path().join("result.json");
    let prototypes = temp.path().join("prototypes.json");
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
        .arg("EquipmentOracle")
        .arg(&input)
        .arg(&output_file)
        .arg(&prototypes));
    let registry: Value = serde_json::from_slice(&std::fs::read(prototypes).unwrap()).unwrap();
    let mut known = 0;
    for (id, value) in registry.as_object().unwrap() {
        if let Some((slot, count, durability)) = native_session::equippable::vanilla_equipment(id) {
            assert_eq!(slot.name(), value[0].as_str().unwrap(), "{id}");
            assert_eq!(u64::from(count), value[1].as_u64().unwrap(), "{id}");
            assert_eq!(
                u64::from(durability.unwrap_or(0)),
                value[2].as_u64().unwrap(),
                "{id}"
            );
            assert_eq!(value[3], false, "{id} overrides ordinary slot clicks");
            known += 1;
        }
    }
    assert_eq!(known, registry.as_object().unwrap().len());
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(output_file).unwrap()).unwrap();
    for (index, case) in cases.iter().enumerate() {
        let creative = case["creative"].as_bool().unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_enchantments(Arc::clone(&enchantments));
        inventory.toggle();
        for i in 0..36 {
            inventory.set_slot(
                PlayerSlot::Storage(i),
                native_item(&case["storage"][i as usize]),
            );
        }
        for i in 0..4 {
            inventory.set_slot(
                PlayerSlot::Armor(i),
                native_item(&case["armor"][i as usize]),
            );
        }
        inventory.set_slot(PlayerSlot::Offhand, native_item(&case["offhand"]));
        if let Some(cursor) = native_item(&case["cursor"]) {
            inventory.set_slot(PlayerSlot::Storage(35), Some(cursor));
            inventory.click(PlayerSlot::Storage(35), ClickButton::Primary);
        }
        let source = case["source"].as_i64().unwrap() as u8;
        let source = if source >= 100 {
            PlayerSlot::Armor(source - 100)
        } else {
            PlayerSlot::Storage(source)
        };
        let button = if case["button"] == 0 {
            ClickButton::Primary
        } else {
            ClickButton::Secondary
        };
        let mut dropped = None;
        match case["operation"].as_str().unwrap() {
            "click" => {
                inventory.click_with_mode(source, button, creative);
            }
            "quick" => {
                inventory.quick_move_with_mode(source, creative);
            }
            "swap" => {
                dropped = inventory
                    .swap_slot(source, PlayerSlot::Storage(0), creative, None)
                    .and_then(|mut values| values.pop());
            }
            "throw" => {
                dropped = inventory.throw_slot_with_mode(
                    source,
                    button == ClickButton::Secondary,
                    creative,
                );
            }
            _ => unreachable!(),
        }
        let storage: Vec<_> = (0..36)
            .map(|i| output(inventory.slot(PlayerSlot::Storage(i))))
            .collect();
        let armor: Vec<_> = (0..4)
            .map(|i| output(inventory.slot(PlayerSlot::Armor(i))))
            .collect();
        let actual = json!([
            storage,
            armor,
            output(inventory.slot(PlayerSlot::Offhand)),
            output(inventory.cursor()),
            output(dropped.as_ref())
        ]);
        assert_eq!(actual, expected[index], "case {index}: {case}");
    }
    println!(
        "Compared {} equipment interactions with Java 26.3",
        cases.len()
    );
}
