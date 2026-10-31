//! Client-side hand state advanced at the same fixed tick rate as the player.
//! Custom swing components, item model swap rules and attack-speed modifiers
//! are not yet resolved; supported basic items use the default 26.3 behavior.

use std::sync::Arc;

use crate::inventory::{ItemStack, PlayerInventory, PlayerSlot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Main,
    Off,
}

/// Item-use state shared with the renderer after a completed simulation tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemUseState {
    pub hand: Hand,
    pub remaining_ticks: i32,
}

#[derive(Clone, Debug, Default)]
pub struct HandState {
    pub item: Option<Arc<ItemStack>>,
    pub old_height: f32,
    pub height: f32,
    held: Option<Arc<ItemStack>>,
    matches_held: bool,
}

impl HandState {
    /// Vanilla passes one minus the interpolated height into the hand pose.
    pub fn equip_progress(&self, partial: f32) -> f32 {
        1.0 - (self.old_height + (self.height - self.old_height) * partial.clamp(0.0, 1.0))
    }

    fn set_held(&mut self, held: Option<&ItemStack>) {
        if self.held.as_deref() != held {
            self.held = held.cloned().map(Arc::new);
        }
        let identical = self.item.as_deref() == held;
        self.matches_held = identical
            || match (self.item.as_deref(), held) {
                (None, None) => true,
                (Some(previous), Some(current)) => {
                    previous == current || previous.same_item_and_components(current)
                }
                _ => false,
            };
        if self.matches_held && !identical {
            self.item = self.held.clone();
        }
    }

    fn tick(&mut self, target_height: f32) {
        self.old_height = self.height;
        let target = if self.matches_held {
            target_height
        } else {
            0.0
        };
        self.height += (target - self.height).clamp(-0.4, 0.4);
        if self.height < 0.1 && !self.matches_held {
            self.item = self.held.clone();
            self.matches_held = true;
        }
    }
}

/// The default WHACK swing, six ticks in the absence of status effects.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SwingState {
    hand: Option<Hand>,
    ticks: u8,
    old_animation: f32,
    animation: f32,
}

impl SwingState {
    pub fn hand(self) -> Option<Hand> {
        self.hand
    }

    pub fn progress(self, partial: f32) -> f32 {
        let end = if self.animation < self.old_animation {
            self.animation + 1.0
        } else {
            self.animation
        };
        self.old_animation + (end - self.old_animation) * partial.clamp(0.0, 1.0)
    }

    pub fn start(&mut self, hand: Hand) -> bool {
        if self.hand.is_none() || self.ticks > 3 || self.ticks == 0 {
            if self.hand != Some(hand) {
                self.old_animation = 0.0;
                self.animation = 0.0;
            }
            self.hand = Some(hand);
            self.ticks = 0;
            true
        } else {
            false
        }
    }

    fn tick(&mut self) {
        self.old_animation = self.animation;
        if self.hand.is_some() {
            self.animation = (f32::from(self.ticks) / 6.0).min(1.0);
            let previous_ticks = self.ticks;
            self.ticks += 1;
            if previous_ticks > 6 {
                self.hand = None;
                self.animation = 0.0;
            }
        } else {
            self.animation = 0.0;
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FirstPersonState {
    pub main: HandState,
    pub off: HandState,
    pub swing: SwingState,
    pub item_use: Option<ItemUseState>,
    inventory_revision: Option<u64>,
}

impl FirstPersonState {
    pub(crate) fn invalidate_inventory(&mut self) {
        self.inventory_revision = None;
    }

    pub(crate) fn used(&mut self, hand: Hand) {
        match hand {
            Hand::Main => self.main.height = 0.0,
            Hand::Off => self.off.height = 0.0,
        }
    }

    pub(crate) fn tick(&mut self, inventory: &PlayerInventory) {
        self.swing.tick();
        if self.inventory_revision != Some(inventory.revision()) {
            self.main
                .set_held(inventory.slot(PlayerSlot::Storage(inventory.selected())));
            self.off.set_held(inventory.slot(PlayerSlot::Offhand));
            self.inventory_revision = Some(inventory.revision());
        }
        self.main.tick(1.0);
        self.off.tick(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::Tag;

    fn stack(id: &str, count: u32) -> ItemStack {
        ItemStack::new(id.into(), count, 64, Tag::Compound(vec![])).unwrap()
    }

    #[test]
    fn displayed_item_lowers_before_switching_and_raises_afterwards() {
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(PlayerSlot::Storage(0), Some(stack("minecraft:stone", 5)));
        inventory.set_slot(PlayerSlot::Storage(1), Some(stack("minecraft:dirt", 5)));
        let mut hands = FirstPersonState::default();
        for _ in 0..4 {
            hands.tick(&inventory);
        }
        assert_eq!(hands.main.item.as_deref().unwrap().id(), "minecraft:stone");
        assert_eq!(hands.main.height, 1.0);
        let retained = Arc::clone(hands.main.item.as_ref().unwrap());
        for _ in 0..5 {
            hands.tick(&inventory);
        }
        assert!(Arc::ptr_eq(&retained, hands.main.item.as_ref().unwrap()));
        inventory.select(1);
        hands.tick(&inventory);
        assert!((hands.main.height - 0.6).abs() < 1e-6);
        assert_eq!(hands.main.item.as_deref().unwrap().id(), "minecraft:stone");
        hands.tick(&inventory);
        assert!((hands.main.height - 0.2).abs() < 1e-6);
        hands.tick(&inventory);
        assert_eq!(hands.main.item.as_deref().unwrap().id(), "minecraft:dirt");
        assert_eq!(hands.main.height, 0.0);
        hands.tick(&inventory);
        assert!((hands.main.equip_progress(0.5) - 0.8).abs() < 1e-6);
        inventory.set_slot(PlayerSlot::Storage(1), Some(stack("minecraft:dirt", 4)));
        hands.tick(&inventory);
        assert!((hands.main.height - 0.8).abs() < 1e-6);
        assert_eq!(hands.main.item.as_deref().unwrap().count(), 4);
    }

    #[test]
    fn default_swing_restarts_only_after_halfway_and_switches_hands() {
        let mut swing = SwingState::default();
        assert!(swing.start(Hand::Main));
        swing.tick();
        assert!(!swing.start(Hand::Off));
        for _ in 0..3 {
            swing.tick();
        }
        assert!(swing.start(Hand::Off));
        assert_eq!(swing.hand(), Some(Hand::Off));
        assert_eq!(swing.progress(0.5), 0.0);
        for _ in 0..8 {
            swing.tick();
        }
        assert_eq!(swing.hand(), None);
        assert!(swing.progress(0.5) > 0.9);
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn default_swing_and_equip_sequences_match_java() {
        use serde_json::Value;
        use std::{
            ffi::OsString,
            path::{Path, PathBuf},
            process::Command,
        };
        const JAVA: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.LivingEntity.SwingState;
import net.minecraft.world.entity.LivingEntity.SwingDescription;
import net.minecraft.world.item.component.SwingAnimation;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.client.player.FirstPersonHandsAndItems;
import net.minecraft.client.renderer.item.ItemModelResolver;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import sun.misc.Unsafe;
public class SwingOracle {
  static class ModelResolver extends ItemModelResolver {
    ModelResolver() { super(null); }
    public boolean shouldPlaySwapAnimation(ItemStack stack) { return true; }
  }
  static class Game extends Minecraft {
    Game() { super(null); }
    public ItemModelResolver getItemModelResolver() { return new ModelResolver(); }
  }
  static class Player extends LocalPlayer {
    ItemStack main=ItemStack.EMPTY, off=ItemStack.EMPTY;
    Player() { super(null,null,null,null,null,null,false,null,null); }
    public ItemStack getMainHandItem() { return main; }
    public ItemStack getOffhandItem() { return off; }
    public boolean isHandsBusy() { return false; }
    public float getItemSwapScale(float partial) { return 1f; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    for(var item:new net.minecraft.world.item.Item[]{Items.STONE,Items.DIRT})
      item.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
    var current=SwingState.class.getDeclaredField("currentSwing"); current.setAccessible(true);
    var out=new JsonArray();
    for(int scenario=0;scenario<2187;scenario++) {
      var state=new SwingState(); int code=scenario;
      for(int tick=0;tick<15;tick++) {
        int action=tick<7?code%3:0; if(tick<7) code/=3; int started=-1;
        if(action!=0) started=state.startIfAble(action==1?InteractionHand.MAIN_HAND:InteractionHand.OFF_HAND,SwingAnimation.DEFAULT,6)?1:0;
        state.tick(); var d=(SwingDescription)current.get(state);
        var row=new JsonArray(); row.add(started); row.add(d==null?0:d.hand()==InteractionHand.MAIN_HAND?1:2);
        for(float partial:new float[]{0f,0.25f,0.5f,0.75f,1f}) row.add(state.getAnimation(partial));
        out.add(row);
      }
    }
    var unsafeField=Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
    var unsafe=(Unsafe)unsafeField.get(null);
    var player=(Player)unsafe.allocateInstance(Player.class);
    var game=(Game)unsafe.allocateInstance(Game.class);
    var minecraft=LocalPlayer.class.getDeclaredField("minecraft"); minecraft.setAccessible(true); minecraft.set(player,game);
    player.main=ItemStack.EMPTY; player.off=ItemStack.EMPTY;
    var stone=new ItemStack(Items.STONE,5); var dirt=new ItemStack(Items.DIRT,5);
    var equip=new FirstPersonHandsAndItems();
    var fields=new java.lang.reflect.Field[6];
    String[] names={"mainHandItem","oMainHandHeight","mainHandHeight","offHandItem","oOffHandHeight","offHandHeight"};
    for(int i=0;i<names.length;i++) { fields[i]=FirstPersonHandsAndItems.class.getDeclaredField(names[i]); fields[i].setAccessible(true); }
    var equipment=new JsonArray();
    for(int tick=0;tick<30;tick++) {
      switch(tick) {
        case 0 -> player.main=stone;
        case 5 -> player.main=dirt;
        case 7 -> player.main=stone;
        case 12 -> player.main=ItemStack.EMPTY;
        case 15 -> player.off=dirt;
        case 18 -> player.main=stone;
        case 22 -> stone.setCount(4);
        case 24 -> equip.itemUsed(InteractionHand.MAIN_HAND);
        case 26 -> player.off=ItemStack.EMPTY;
      }
      equip.tick(player);
      var row=new JsonArray();
      for(var f:fields) {
        if(f.getType()==ItemStack.class) {
          var item=(ItemStack)f.get(equip);
          row.add(item.isEmpty()?"":BuiltInRegistries.ITEM.getKey(item.getItem()).toString());
          row.add(item.getCount());
        } else row.add(f.getFloat(equip));
      }
      equipment.add(row);
    }
    var result=new JsonObject(); result.add("swings",out); result.add("equipment",equipment);
    Files.writeString(Path.of(args[0]),new Gson().toJson(result));
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
                "swing oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("SwingOracle.java");
        let result = temp.path().join("output.json");
        std::fs::write(&source, JAVA).unwrap();
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
            .arg("SwingOracle")
            .arg(&result));
        let result: Value = serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        let rows = result["swings"].as_array().unwrap();
        let mut index = 0;
        for scenario in 0..2187 {
            let mut state = SwingState::default();
            let mut code = scenario;
            for tick in 0..15 {
                let action = if tick < 7 { code % 3 } else { 0 };
                if tick < 7 {
                    code /= 3;
                }
                let started = match action {
                    1 => i64::from(state.start(Hand::Main)),
                    2 => i64::from(state.start(Hand::Off)),
                    _ => -1,
                };
                state.tick();
                let expected = &rows[index];
                index += 1;
                assert_eq!(
                    started,
                    expected[0].as_i64().unwrap(),
                    "scenario={scenario} tick={tick}"
                );
                let hand = match state.hand() {
                    None => 0,
                    Some(Hand::Main) => 1,
                    Some(Hand::Off) => 2,
                };
                assert_eq!(
                    hand,
                    expected[1].as_i64().unwrap(),
                    "scenario={scenario} tick={tick}"
                );
                for (i, partial) in [0.0, 0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
                    let value = state.progress(partial);
                    let java = expected[2 + i].as_f64().unwrap() as f32;
                    assert!(
                        (value - java).abs() < 1e-6,
                        "scenario={scenario} tick={tick} partial={partial}: {value} != {java}"
                    );
                }
            }
        }
        assert_eq!(index, rows.len());
        let mut inventory = PlayerInventory::default();
        let mut hands = FirstPersonState::default();
        let rows = result["equipment"].as_array().unwrap();
        assert_eq!(rows.len(), 30);
        for (tick, row) in rows.iter().enumerate() {
            match tick {
                0 | 7 | 18 => {
                    inventory.set_slot(PlayerSlot::Storage(0), Some(stack("minecraft:stone", 5)));
                }
                5 => {
                    inventory.set_slot(PlayerSlot::Storage(0), Some(stack("minecraft:dirt", 5)));
                }
                12 => {
                    inventory.set_slot(PlayerSlot::Storage(0), None);
                }
                15 => {
                    inventory.set_slot(PlayerSlot::Offhand, Some(stack("minecraft:dirt", 5)));
                }
                22 => {
                    inventory.set_slot(PlayerSlot::Storage(0), Some(stack("minecraft:stone", 4)));
                }
                24 => hands.used(Hand::Main),
                26 => {
                    inventory.set_slot(PlayerSlot::Offhand, None);
                }
                _ => {}
            }
            hands.tick(&inventory);
            for (hand, offset) in [(&hands.main, 0), (&hands.off, 4)] {
                let item = hand.item.as_deref();
                assert_eq!(
                    item.map_or("", ItemStack::id),
                    row[offset].as_str().unwrap(),
                    "tick={tick}"
                );
                assert_eq!(
                    u64::from(item.map_or(0, ItemStack::count)),
                    row[offset + 1].as_u64().unwrap(),
                    "tick={tick}"
                );
                for (actual, index) in [(hand.old_height, offset + 2), (hand.height, offset + 3)] {
                    let java = row[index].as_f64().unwrap() as f32;
                    assert!(
                        (actual - java).abs() < 1e-6,
                        "tick={tick} {actual} != {java}"
                    );
                }
            }
        }
    }
}
