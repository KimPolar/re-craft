//! Client-side hand state advanced at the same fixed tick rate as the player.
//! Some item-model swap rules are not yet resolved.

use std::sync::Arc;

use crate::inventory::{ItemStack, PlayerInventory, PlayerSlot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Main,
    Off,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SwingKind {
    None,
    #[default]
    Whack,
    Stab,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SwingAnimation {
    kind: SwingKind,
    duration: i32,
}

impl Default for SwingAnimation {
    fn default() -> Self {
        Self {
            kind: SwingKind::Whack,
            duration: 6,
        }
    }
}

/// Explicit components override the 26.3 prototype; a removal uses Java's
/// fallback, even when the prototype supplies an animation.
fn item_swing_animation(item: Option<&ItemStack>, component: &str) -> SwingAnimation {
    let Some(item) = item else {
        return SwingAnimation::default();
    };
    if let Some(value) = item.components().get(component) {
        return decode_swing_animation(value).unwrap_or_default();
    }
    if component == "minecraft:attack_animation"
        && item
            .components()
            .get("!minecraft:attack_animation")
            .is_none()
    {
        let duration = match item.id() {
            "minecraft:wooden_spear" => 13,
            "minecraft:stone_spear" => 15,
            "minecraft:copper_spear" => 17,
            "minecraft:iron_spear" | "minecraft:golden_spear" => 19,
            "minecraft:diamond_spear" => 21,
            "minecraft:netherite_spear" => 23,
            _ => return SwingAnimation::default(),
        };
        return SwingAnimation {
            kind: SwingKind::Stab,
            duration,
        };
    }
    SwingAnimation::default()
}

fn decode_swing_animation(value: &native_persistence::nbt::Tag) -> Option<SwingAnimation> {
    use native_persistence::nbt::Tag;
    let Tag::Compound(fields) = value else {
        return None;
    };
    let mut animation = SwingAnimation::default();
    let (mut seen_type, mut seen_duration) = (false, false);
    for (name, value) in fields {
        if name.matches("type") {
            if seen_type {
                return None;
            }
            seen_type = true;
            animation.kind = match value {
                Tag::String(value) if value.matches("none") => SwingKind::None,
                Tag::String(value) if value.matches("whack") => SwingKind::Whack,
                Tag::String(value) if value.matches("stab") => SwingKind::Stab,
                _ => return None,
            };
        } else if name.matches("duration") {
            if seen_duration {
                return None;
            }
            seen_duration = true;
            animation.duration = value.as_java_int().filter(|duration| *duration >= 0)?;
        }
    }
    Some(animation)
}

pub(crate) fn valid_swing_component(value: &native_persistence::nbt::Tag) -> bool {
    decode_swing_animation(value).is_some()
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

/// Current swing animation and its effective duration at the time it started.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SwingState {
    hand: Option<Hand>,
    ticks: i32,
    duration: i32,
    animation_type: SwingKind,
    base_duration: i32,
    old_animation: f32,
    animation: f32,
}

impl SwingState {
    pub fn hand(self) -> Option<Hand> {
        self.hand
    }

    pub fn kind(self) -> Option<SwingKind> {
        self.hand.map(|_| self.animation_type)
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
        self.start_with_duration(hand, 6)
    }

    fn start_with_duration(&mut self, hand: Hand, duration: i32) -> bool {
        self.start_with_animation(hand, SwingAnimation::default(), duration)
    }

    fn start_with_animation(
        &mut self,
        hand: Hand,
        animation: SwingAnimation,
        duration: i32,
    ) -> bool {
        if self.hand.is_none() || self.ticks > self.duration / 2 || self.ticks <= 0 {
            if self.hand != Some(hand)
                || self.duration != duration
                || self.animation_type != animation.kind
                || self.base_duration != animation.duration
            {
                self.old_animation = 0.0;
                self.animation = 0.0;
            }
            self.hand = Some(hand);
            self.duration = duration;
            self.animation_type = animation.kind;
            self.base_duration = animation.duration;
            self.ticks = 0;
            true
        } else {
            false
        }
    }

    fn tick(&mut self) {
        self.old_animation = self.animation;
        if self.hand.is_some() {
            if self.duration > 0 {
                self.animation = (self.ticks as f32 / self.duration as f32).min(1.0);
            }
            let previous_ticks = self.ticks;
            self.ticks = self.ticks.wrapping_add(1);
            if previous_ticks > self.duration {
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
    /// HUD charge at the end of the last player tick, absent if unresolved.
    pub attack_strength: Option<f32>,
    inventory_revision: Option<u64>,
    interaction_animations: Option<[SwingAnimation; 2]>,
}

impl FirstPersonState {
    /// Capture both hands before an interaction can consume or replace an item.
    pub(crate) fn begin_interaction(&mut self, inventory: &PlayerInventory) {
        self.interaction_animations = Some([
            item_swing_animation(
                inventory.slot(PlayerSlot::Storage(inventory.selected())),
                "minecraft:interact_animation",
            ),
            item_swing_animation(
                inventory.slot(PlayerSlot::Offhand),
                "minecraft:interact_animation",
            ),
        ]);
    }

    pub(crate) fn end_interaction(&mut self) {
        self.interaction_animations = None;
    }

    /// Resolve effect modifiers when the swing starts, as LivingEntity does.
    pub(crate) fn start_swing(
        &mut self,
        hand: Hand,
        effects: &crate::survival::FallEffects,
        breathing: &crate::survival::BreathingEffects,
    ) -> bool {
        let animation =
            self.interaction_animations
                .map_or_else(SwingAnimation::default, |values| {
                    values[match hand {
                        Hand::Main => 0,
                        Hand::Off => 1,
                    }]
                });
        self.start_animation(hand, animation, effects, breathing)
    }

    pub(crate) fn start_attack_swing(
        &mut self,
        hand: Hand,
        item: Option<&ItemStack>,
        effects: &crate::survival::FallEffects,
        breathing: &crate::survival::BreathingEffects,
    ) -> bool {
        self.start_animation(
            hand,
            item_swing_animation(item, "minecraft:attack_animation"),
            effects,
            breathing,
        )
    }

    fn start_animation(
        &mut self,
        hand: Hand,
        animation: SwingAnimation,
        effects: &crate::survival::FallEffects,
        breathing: &crate::survival::BreathingEffects,
    ) -> bool {
        let dig_speed = effects
            .haste
            .iter()
            .chain(breathing.conduit_power.iter())
            .map(|effect| effect.amplifier)
            .max();
        let duration = if let Some(amplifier) = dig_speed {
            animation.duration.wrapping_sub(1 + i32::from(amplifier))
        } else if let Some(effect) = &effects.mining_fatigue {
            animation
                .duration
                .wrapping_add((1 + i32::from(effect.amplifier)) * 2)
        } else {
            animation.duration
        };
        self.swing.start_with_animation(hand, animation, duration)
    }

    pub(crate) fn invalidate_inventory(&mut self) {
        self.inventory_revision = None;
    }

    pub(crate) fn used(&mut self, hand: Hand) {
        match hand {
            Hand::Main => self.main.height = 0.0,
            Hand::Off => self.off.height = 0.0,
        }
    }

    #[cfg(test)]
    fn tick(&mut self, inventory: &PlayerInventory) {
        self.tick_with_swap(inventory, 1.0);
    }

    pub(crate) fn tick_with_swap(&mut self, inventory: &PlayerInventory, swap: f32) {
        self.swing.tick();
        if self.inventory_revision != Some(inventory.revision()) {
            self.main
                .set_held(inventory.slot(PlayerSlot::Storage(inventory.selected())));
            self.off.set_held(inventory.slot(PlayerSlot::Offhand));
            self.inventory_revision = Some(inventory.revision());
        }
        self.main.tick(swap * swap * swap);
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
    fn swing_effects_use_strongest_speed_and_capture_duration_at_start() {
        let effect = |amplifier| crate::survival::EffectDuration {
            amplifier,
            duration: 100,
            ambient: false,
            visible: true,
            show_icon: true,
            hidden: None,
        };
        let mut effects = crate::survival::FallEffects {
            haste: Some(effect(1)),
            mining_fatigue: Some(effect(255)),
            ..Default::default()
        };
        let breathing = crate::survival::BreathingEffects {
            conduit_power: Some(effect(3)),
            ..Default::default()
        };
        let mut hands = FirstPersonState::default();
        assert!(hands.start_swing(Hand::Main, &effects, &breathing));
        assert_eq!(hands.swing.duration, 2);
        effects.haste = None;
        hands.swing.tick();
        assert!(!hands.start_swing(Hand::Off, &effects, &Default::default()));
        assert_eq!(hands.swing.duration, 2);
        hands.swing.tick();
        assert!(hands.start_swing(Hand::Off, &effects, &Default::default()));
        assert_eq!(hands.swing.duration, 518);
        for _ in 0..519 {
            hands.swing.tick();
        }
        assert!(hands.swing.hand().is_some());
        hands.swing.tick();
        assert!(hands.swing.hand().is_none());
    }

    #[test]
    fn swing_components_keep_ordinary_items_usable_without_accepting_invalid_patches() {
        let component = |name: &str, value: Tag| Tag::Compound(vec![(name.into(), value)]);
        for name in ["minecraft:attack_animation", "minecraft:interact_animation"] {
            for kind in ["none", "whack", "stab"] {
                let value = Tag::Compound(vec![
                    ("type".into(), Tag::String(kind.into())),
                    ("duration".into(), Tag::Int(12)),
                ]);
                assert_eq!(
                    crate::inventory::saved_item_max("minecraft:dirt", &component(name, value)),
                    Some(64)
                );
            }
            for invalid in [
                Tag::Int(1),
                Tag::Compound(vec![("type".into(), Tag::String("unknown".into()))]),
                Tag::Compound(vec![("duration".into(), Tag::Int(-1))]),
                Tag::Compound(vec![
                    ("duration".into(), Tag::Int(1)),
                    ("duration".into(), Tag::Int(2)),
                ]),
            ] {
                assert_eq!(
                    crate::inventory::saved_item_max("minecraft:dirt", &component(name, invalid)),
                    None
                );
            }
            let removed = format!("!{name}");
            assert_eq!(
                crate::inventory::saved_item_max(
                    "minecraft:dirt",
                    &Tag::Compound(vec![
                        (name.into(), Tag::Compound(vec![])),
                        (removed.as_str().into(), Tag::Compound(vec![])),
                    ])
                ),
                None
            );
        }
    }

    #[test]
    fn spear_default_can_be_removed_or_overridden() {
        for (id, duration, durability) in [
            ("minecraft:wooden_spear", 13, 59),
            ("minecraft:stone_spear", 15, 131),
            ("minecraft:copper_spear", 17, 190),
            ("minecraft:iron_spear", 19, 250),
            ("minecraft:golden_spear", 19, 32),
            ("minecraft:diamond_spear", 21, 1561),
            ("minecraft:netherite_spear", 23, 2031),
        ] {
            let make = |components| ItemStack::new(id.into(), 1, 1, components).unwrap();
            let default = make(Tag::Compound(vec![]));
            assert_eq!(
                item_swing_animation(Some(&default), "minecraft:attack_animation"),
                SwingAnimation {
                    kind: SwingKind::Stab,
                    duration
                }
            );
            assert_eq!(
                item_swing_animation(Some(&default), "minecraft:interact_animation"),
                SwingAnimation::default()
            );
            assert_eq!(crate::inventory::ordinary_item_default_max(id), Some(1));
            assert_eq!(
                crate::inventory::ordinary_item_default_durability(id),
                Some(durability)
            );
            let removed = make(Tag::Compound(vec![(
                "!minecraft:attack_animation".into(),
                Tag::Compound(vec![]),
            )]));
            assert_eq!(
                item_swing_animation(Some(&removed), "minecraft:attack_animation"),
                SwingAnimation::default()
            );
            let overridden = make(Tag::Compound(vec![(
                "minecraft:attack_animation".into(),
                Tag::Compound(vec![
                    ("type".into(), Tag::String("none".into())),
                    ("duration".into(), Tag::Int(11)),
                ]),
            )]));
            assert_eq!(
                item_swing_animation(Some(&overridden), "minecraft:attack_animation"),
                SwingAnimation {
                    kind: SwingKind::None,
                    duration: 11
                }
            );
        }
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
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.ai.attributes.Attributes;
import net.minecraft.world.item.component.SwingAnimation;
import net.minecraft.world.item.SwingAnimationType;
import net.minecraft.world.item.component.PiercingWeapon;
import net.minecraft.client.multiplayer.MultiPlayerGameMode;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.effect.MobEffects;
import net.minecraft.core.Holder;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
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
    float swapScale;
    Player() { super(null,null,null,null,null,null,false,null,null); }
    public ItemStack getMainHandItem() { return main; }
    public ItemStack getOffhandItem() { return off; }
    public boolean isHandsBusy() { return false; }
    public float getItemSwapScale(float partial) { return swapScale; }
  }
  static class EffectPlayer extends Player {
    MobEffectInstance haste, conduit, fatigue;
    public boolean hasEffect(Holder<MobEffect> effect) { return getEffect(effect)!=null; }
    public MobEffectInstance getEffect(Holder<MobEffect> effect) {
      if(effect==MobEffects.HASTE) return haste;
      if(effect==MobEffects.CONDUIT_POWER) return conduit;
      if(effect==MobEffects.MINING_FATIGUE) return fatigue;
      return null;
    }
  }
  static class AttackPlayer extends Player {
    ItemStack held;
    public ItemStack getItemInHand(InteractionHand hand) { return held; }
    public boolean isUsingItem() { return false; }
  }
  static class ChargePlayer extends Player {
    double speed;
    public double getAttributeValue(Holder<net.minecraft.world.entity.ai.attributes.Attribute> attribute) { return speed; }
  }
  static class SwapPlayer extends LocalPlayer {
    double speed;
    SwapPlayer() { super(null,null,null,null,null,null,false,null,null); }
    public double getAttributeValue(Holder<net.minecraft.world.entity.ai.attributes.Attribute> attribute) { return speed; }
  }
  static class AttackMode extends MultiPlayerGameMode {
    int stops;
    AttackMode() { super(null,null); }
    public void stopDestroyBlock() { stops++; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registries=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var prototypes=new JsonArray();
    var prototypePatches=new JsonArray();
    for(var item:BuiltInRegistries.ITEM) {
      var stack=new ItemStack(item,1); var attack=stack.getAttackAnimation(); var interact=stack.getInteractAnimation();
      var row=new JsonArray(); row.add(BuiltInRegistries.ITEM.getKey(item).toString());
      row.add(attack.type().getSerializedName()); row.add(attack.duration());
      row.add(interact.type().getSerializedName()); row.add(interact.duration());
      row.add(stack.getMaxStackSize()); row.add(stack.getMaxDamage()); row.add(stack.has(DataComponents.PIERCING_WEAPON));
      var speed=new double[1]; var hasSpeed=new boolean[1];
      stack.forEachModifier(EquipmentSlot.MAINHAND,(attribute,modifier) -> {
        if(attribute==Attributes.ATTACK_SPEED) { hasSpeed[0]=true; speed[0]+=modifier.amount(); }
      });
      if(hasSpeed[0]) row.add(speed[0]); else row.add((Number)null);
      row.add(stack.getOrDefault(DataComponents.MINIMUM_ATTACK_CHARGE,0f));
      prototypes.add(row);
      if(attack.type()==SwingAnimationType.STAB) {
        stack.remove(DataComponents.ATTACK_ANIMATION); var removed=stack.getAttackAnimation();
        stack.set(DataComponents.ATTACK_ANIMATION,new SwingAnimation(SwingAnimationType.NONE,11)); var overridden=stack.getAttackAnimation();
        var patch=new JsonArray(); patch.add(BuiltInRegistries.ITEM.getKey(item).toString());
        patch.add(removed.type().getSerializedName()); patch.add(removed.duration());
        patch.add(overridden.type().getSerializedName()); patch.add(overridden.duration());
        stack.remove(DataComponents.PIERCING_WEAPON); patch.add(stack.has(DataComponents.PIERCING_WEAPON));
        prototypePatches.add(patch);
      }
    }
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
    var swapPlayer=(SwapPlayer)unsafe.allocateInstance(SwapPlayer.class);
    var swapTicker=net.minecraft.world.entity.LivingEntity.class.getDeclaredField("itemSwapTicker"); swapTicker.setAccessible(true);
    var swaps=new JsonArray();
    for(double speed:new double[]{0,0.1,1,4,1024})
    for(int ticks:new int[]{-1,0,1,4,19,20,21,100})
    for(float partial:new float[]{0f,0.25f,1f}) {
      swapPlayer.speed=speed; swapTicker.setInt(swapPlayer,ticks);
      var row=new JsonArray(); row.add(speed); row.add(ticks); row.add(partial); row.add(swapPlayer.getItemSwapScale(partial)); swaps.add(row);
    }
    var chargePlayer=(ChargePlayer)unsafe.allocateInstance(ChargePlayer.class);
    var ticker=net.minecraft.world.entity.LivingEntity.class.getDeclaredField("attackStrengthTicker"); ticker.setAccessible(true);
    var charges=new JsonArray();
    for(double speed:new double[]{0,0.1,1,4,1024})
    for(float minimum:new float[]{0f,0.25f,0.5f,1f})
    for(int ticks:new int[]{-1,0,1,4,19,20,21,100}) {
      chargePlayer.speed=speed; ticker.setInt(chargePlayer,ticks);
      var stack=new ItemStack(Items.STONE,1); stack.set(DataComponents.MINIMUM_ATTACK_CHARGE,minimum);
      var row=new JsonArray(); row.add(speed); row.add(minimum); row.add(ticks);
      row.add(chargePlayer.cannotAttackWithItem(stack,0)); row.add(chargePlayer.getAttackStrengthScale(0f)); charges.add(row);
    }
    var attackPlayer=(AttackPlayer)unsafe.allocateInstance(AttackPlayer.class);
    var attackMode=(AttackMode)unsafe.allocateInstance(AttackMode.class);
    var attackGame=(Game)unsafe.allocateInstance(Game.class);
    attackGame.player=attackPlayer; attackGame.gameMode=attackMode;
    var continueAttack=Minecraft.class.getDeclaredMethod("continueAttack",boolean.class); continueAttack.setAccessible(true);
    var routes=new JsonArray();
    for(int variant=0;variant<4;variant++) {
      var stack=new ItemStack(variant==0||variant==1?Items.IRON_SPEAR:Items.STONE,1);
      if(variant==1) stack.remove(DataComponents.PIERCING_WEAPON);
      if(variant==3) stack.set(DataComponents.PIERCING_WEAPON,new PiercingWeapon(false,false,java.util.Optional.empty(),java.util.Optional.empty()));
      attackPlayer.held=stack; var row=new JsonArray(); row.add(stack.has(DataComponents.PIERCING_WEAPON));
      for(boolean held:new boolean[]{false,true}) { attackMode.stops=0; continueAttack.invoke(attackGame,held); row.add(attackMode.stops); }
      routes.add(row);
    }
    var effectPlayer=(EffectPlayer)unsafe.allocateInstance(EffectPlayer.class);
    var duration=net.minecraft.world.entity.LivingEntity.class.getDeclaredMethod("getModifiedSwingDuration",SwingAnimation.class);
    duration.setAccessible(true);
    var modified=new JsonArray();
    int[] amplifiers={-1,0,1,5,6,255};
    for(int haste:amplifiers) for(int conduit:amplifiers) for(int fatigue:amplifiers) {
      var state=new SwingState();
      var rows=new JsonArray();
      for(int tick=0;tick<18;tick++) {
        int h=(tick>=7 && tick<14)?-1:haste;
        int c=(tick>=7 && tick<14)?-1:conduit;
        effectPlayer.haste=h<0?null:new MobEffectInstance(MobEffects.HASTE,100,h);
        effectPlayer.conduit=c<0?null:new MobEffectInstance(MobEffects.CONDUIT_POWER,100,c);
        effectPlayer.fatigue=fatigue<0?null:new MobEffectInstance(MobEffects.MINING_FATIGUE,100,fatigue);
        int d=(int)duration.invoke(effectPlayer,SwingAnimation.DEFAULT);
        int started=-1;
        if(tick%3==0) started=state.startIfAble(tick%2==0?InteractionHand.MAIN_HAND:InteractionHand.OFF_HAND,SwingAnimation.DEFAULT,d)?1:0;
        state.tick(); var swing=(SwingDescription)current.get(state);
        var row=new JsonArray(); row.add(d); row.add(started); row.add(swing==null?0:swing.hand()==InteractionHand.MAIN_HAND?1:2);
        for(float partial:new float[]{0f,0.25f,0.5f,0.75f,1f}) row.add(state.getAnimation(partial));
        rows.add(row);
      }
      modified.add(rows);
    }
    var custom=new JsonArray();
    for(var kind:new SwingAnimationType[]{SwingAnimationType.NONE,SwingAnimationType.WHACK,SwingAnimationType.STAB})
    for(int ticks:new int[]{0,1,6,13,255,Integer.MAX_VALUE}) {
      var stack=new ItemStack(Items.STONE,1);
      stack.set(DataComponents.ATTACK_ANIMATION,new SwingAnimation(kind,ticks));
      var state=new SwingState(); var rows=new JsonArray();
      for(int tick=0;tick<18;tick++) {
        var animation=stack.getAttackAnimation();
        int started=-1;
        if(tick%3==0) started=state.startIfAble(InteractionHand.MAIN_HAND,animation,animation.duration())?1:0;
        state.tick(); var swing=(SwingDescription)current.get(state);
        var row=new JsonArray(); row.add(started); row.add(swing==null?"":swing.animation().type().getSerializedName());
        for(float partial:new float[]{0f,0.25f,0.5f,0.75f,1f}) row.add(state.getAnimation(partial));
        rows.add(row);
      }
      custom.add(rows);
    }
    var interact=new JsonArray();
    for(var kind:new SwingAnimationType[]{SwingAnimationType.NONE,SwingAnimationType.WHACK,SwingAnimationType.STAB})
    for(int ticks:new int[]{0,1,6,13,255,Integer.MAX_VALUE})
    for(var hand:new InteractionHand[]{InteractionHand.MAIN_HAND,InteractionHand.OFF_HAND}) {
      var stack=new ItemStack(Items.STONE,1);
      stack.set(DataComponents.ATTACK_ANIMATION,new SwingAnimation(SwingAnimationType.WHACK,3));
      stack.set(DataComponents.INTERACT_ANIMATION,new SwingAnimation(kind,ticks));
      var selected=stack.getInteractAnimation();
      stack.shrink(1);
      var state=new SwingState(); var rows=new JsonArray();
      for(int tick=0;tick<18;tick++) {
        int started=-1;
        if(tick%3==0) started=state.startIfAble(hand,selected,selected.duration())?1:0;
        state.tick(); var swing=(SwingDescription)current.get(state);
        var row=new JsonArray(); row.add(started); row.add(swing==null?"":swing.animation().type().getSerializedName());
        for(float partial:new float[]{0f,0.25f,0.5f,0.75f,1f}) row.add(state.getAnimation(partial));
        rows.add(row);
      }
      interact.add(rows);
    }
    var player=(Player)unsafe.allocateInstance(Player.class);
    var game=(Game)unsafe.allocateInstance(Game.class);
    var minecraft=LocalPlayer.class.getDeclaredField("minecraft"); minecraft.setAccessible(true); minecraft.set(player,game);
    player.main=ItemStack.EMPTY; player.off=ItemStack.EMPTY; player.swapScale=1f;
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
    player.main=ItemStack.EMPTY; player.off=ItemStack.EMPTY;
    var scaledEquip=new FirstPersonHandsAndItems(); var scaled=new JsonArray();
    for(int tick=0;tick<24;tick++) {
      if(tick==0) player.main=stone;
      if(tick==12) player.main=dirt;
      if(tick==15) player.off=stone;
      player.swapScale=(tick%12)/11f;
      scaledEquip.tick(player);
      var row=new JsonArray(); row.add(player.swapScale);
      for(var f:fields) {
        if(f.getType()==ItemStack.class) {
          var item=(ItemStack)f.get(scaledEquip);
          row.add(item.isEmpty()?"":BuiltInRegistries.ITEM.getKey(item.getItem()).toString());
          row.add(item.getCount());
        } else row.add(f.getFloat(scaledEquip));
      }
      scaled.add(row);
    }
    var result=new JsonObject(); result.add("swings",out); result.add("equipment",equipment); result.add("scaledEquipment",scaled); result.add("swapScales",swaps); result.add("modified",modified); result.add("custom",custom); result.add("interact",interact); result.add("prototypes",prototypes); result.add("prototypePatches",prototypePatches); result.add("piercingRoutes",routes); result.add("charges",charges);
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
        let parse_kind = |value: &Value| match value.as_str().unwrap() {
            "none" => SwingKind::None,
            "whack" => SwingKind::Whack,
            "stab" => SwingKind::Stab,
            other => panic!("unknown Java swing kind {other}"),
        };
        let prototypes = result["prototypes"].as_array().unwrap();
        for row in prototypes {
            let id = row[0].as_str().unwrap();
            let stack = ItemStack::new(id.into(), 1, 1, Tag::Compound(vec![])).unwrap();
            for (component, offset) in [
                ("minecraft:attack_animation", 1),
                ("minecraft:interact_animation", 3),
            ] {
                let actual = item_swing_animation(Some(&stack), component);
                assert_eq!(actual.kind, parse_kind(&row[offset]), "{id} {component}");
                assert_eq!(
                    i64::from(actual.duration),
                    row[offset + 1].as_i64().unwrap(),
                    "{id} {component}"
                );
            }
            assert_eq!(
                crate::inventory::has_piercing_weapon(Some(&stack)),
                row[7].as_bool().unwrap(),
                "{id} piercing prototype"
            );
            assert_eq!(
                crate::inventory::vanilla_attack_speed_modifier(id),
                row[8].as_f64(),
                "{id} attack-speed prototype"
            );
            assert_eq!(
                crate::combat::minimum_charge(Some(&stack)),
                row[9].as_f64().unwrap() as f32,
                "{id} minimum charge"
            );
            if id.strip_prefix("minecraft:").is_some_and(|name| {
                matches!(
                    name,
                    "wooden_spear"
                        | "stone_spear"
                        | "copper_spear"
                        | "iron_spear"
                        | "golden_spear"
                        | "diamond_spear"
                        | "netherite_spear"
                )
            }) {
                assert_eq!(
                    crate::inventory::ordinary_item_default_max(id).map(u64::from),
                    row[5].as_u64(),
                    "{id} max stack"
                );
                assert_eq!(
                    crate::inventory::ordinary_item_default_durability(id).map(u64::from),
                    row[6].as_u64(),
                    "{id} durability"
                );
            }
        }
        for row in result["charges"].as_array().unwrap() {
            let speed = row[0].as_f64().unwrap();
            let minimum = row[1].as_f64().unwrap() as f32;
            let ticks = row[2].as_i64().unwrap() as i32;
            let stack = ItemStack::new(
                "minecraft:stone".into(),
                1,
                64,
                Tag::Compound(vec![(
                    "minecraft:minimum_attack_charge".into(),
                    Tag::Float(minimum),
                )]),
            )
            .unwrap();
            assert_eq!(
                !crate::combat::AttackCharge::for_test(ticks, speed).can_attack(Some(&stack)),
                row[3].as_bool().unwrap(),
                "{speed} {minimum} {ticks}"
            );
            assert_eq!(
                crate::combat::AttackCharge::for_test(ticks, speed).attack_strength(),
                Some(row[4].as_f64().unwrap() as f32)
            );
        }
        let patches = result["prototypePatches"].as_array().unwrap();
        assert_eq!(patches.len(), 7);
        for row in patches {
            let id = row[0].as_str().unwrap();
            for (components, offset) in [
                (
                    Tag::Compound(vec![(
                        "!minecraft:attack_animation".into(),
                        Tag::Compound(vec![]),
                    )]),
                    1,
                ),
                (
                    Tag::Compound(vec![(
                        "minecraft:attack_animation".into(),
                        Tag::Compound(vec![
                            ("type".into(), Tag::String("none".into())),
                            ("duration".into(), Tag::Int(11)),
                        ]),
                    )]),
                    3,
                ),
            ] {
                let stack = ItemStack::new(id.into(), 1, 1, components).unwrap();
                let actual = item_swing_animation(Some(&stack), "minecraft:attack_animation");
                assert_eq!(actual.kind, parse_kind(&row[offset]), "{id} patch");
                assert_eq!(
                    i64::from(actual.duration),
                    row[offset + 1].as_i64().unwrap(),
                    "{id} patch"
                );
            }
            let removed = ItemStack::new(
                id.into(),
                1,
                1,
                Tag::Compound(vec![(
                    "!minecraft:piercing_weapon".into(),
                    Tag::Compound(vec![]),
                )]),
            )
            .unwrap();
            assert_eq!(
                crate::inventory::has_piercing_weapon(Some(&removed)),
                row[5].as_bool().unwrap()
            );
        }
        let routes = result["piercingRoutes"].as_array().unwrap();
        let cases = [
            ("minecraft:iron_spear", Tag::Compound(vec![])),
            (
                "minecraft:iron_spear",
                Tag::Compound(vec![(
                    "!minecraft:piercing_weapon".into(),
                    Tag::Compound(vec![]),
                )]),
            ),
            ("minecraft:stone", Tag::Compound(vec![])),
            (
                "minecraft:stone",
                Tag::Compound(vec![(
                    "minecraft:piercing_weapon".into(),
                    Tag::Compound(vec![]),
                )]),
            ),
        ];
        assert_eq!(routes.len(), cases.len());
        for ((id, components), row) in cases.into_iter().zip(routes) {
            let stack = ItemStack::new(id.into(), 1, 1, components).unwrap();
            let piercing = crate::inventory::has_piercing_weapon(Some(&stack));
            assert_eq!(piercing, row[0].as_bool().unwrap(), "{id}");
            for held in [1, 2] {
                assert_eq!(
                    i64::from(!piercing),
                    row[held].as_i64().unwrap(),
                    "{id} held={held}"
                );
            }
        }
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
        let modified = result["modified"].as_array().unwrap();
        let effect = |amplifier: i32| {
            (amplifier >= 0).then(|| crate::survival::EffectDuration {
                amplifier: amplifier as u8,
                duration: 100,
                ambient: false,
                visible: true,
                show_icon: true,
                hidden: None,
            })
        };
        let mut scenario = 0;
        for haste in [-1, 0, 1, 5, 6, 255] {
            for conduit in [-1, 0, 1, 5, 6, 255] {
                for fatigue in [-1, 0, 1, 5, 6, 255] {
                    let mut hands = FirstPersonState::default();
                    let rows = modified[scenario].as_array().unwrap();
                    scenario += 1;
                    for (tick, row) in rows.iter().enumerate() {
                        let has_speed = !(7..14).contains(&tick);
                        let effects = crate::survival::FallEffects {
                            haste: effect(if has_speed { haste } else { -1 }),
                            mining_fatigue: effect(fatigue),
                            ..Default::default()
                        };
                        let breathing = crate::survival::BreathingEffects {
                            conduit_power: effect(if has_speed { conduit } else { -1 }),
                            ..Default::default()
                        };
                        let started = if tick % 3 == 0 {
                            let hand = if tick % 2 == 0 { Hand::Main } else { Hand::Off };
                            let started = hands.start_swing(hand, &effects, &breathing);
                            if started {
                                assert_eq!(
                                    i64::from(hands.swing.duration),
                                    row[0].as_i64().unwrap()
                                );
                            }
                            i64::from(started)
                        } else {
                            -1
                        };
                        hands.swing.tick();
                        assert_eq!(
                            started,
                            row[1].as_i64().unwrap(),
                            "scenario={scenario} tick={tick}"
                        );
                        let hand = match hands.swing.hand() {
                            None => 0,
                            Some(Hand::Main) => 1,
                            Some(Hand::Off) => 2,
                        };
                        assert_eq!(
                            hand,
                            row[2].as_i64().unwrap(),
                            "scenario={scenario} tick={tick}"
                        );
                        for (i, partial) in [0.0, 0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
                            let actual = hands.swing.progress(partial);
                            let expected = row[3 + i].as_f64().unwrap() as f32;
                            assert!(
                                (actual - expected).abs() < 1e-6,
                                "scenario={scenario} tick={tick} partial={partial}: {actual} != {expected}"
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(scenario, modified.len());
        let custom = result["custom"].as_array().unwrap();
        let mut scenario = 0;
        for (name, kind) in [
            ("none", SwingKind::None),
            ("whack", SwingKind::Whack),
            ("stab", SwingKind::Stab),
        ] {
            for duration in [0, 1, 6, 13, 255, i32::MAX] {
                let stack = ItemStack::new(
                    "minecraft:stone".into(),
                    1,
                    64,
                    Tag::Compound(vec![(
                        "minecraft:attack_animation".into(),
                        Tag::Compound(vec![
                            ("type".into(), Tag::String(name.into())),
                            ("duration".into(), Tag::Int(duration)),
                        ]),
                    )]),
                )
                .unwrap();
                let mut hands = FirstPersonState::default();
                for (tick, row) in custom[scenario].as_array().unwrap().iter().enumerate() {
                    let started = if tick % 3 == 0 {
                        i64::from(hands.start_attack_swing(
                            Hand::Main,
                            Some(&stack),
                            &Default::default(),
                            &Default::default(),
                        ))
                    } else {
                        -1
                    };
                    hands.swing.tick();
                    assert_eq!(
                        started,
                        row[0].as_i64().unwrap(),
                        "{name} duration={duration} tick={tick}"
                    );
                    assert_eq!(
                        hands.swing.kind(),
                        if row[1].as_str().unwrap().is_empty() {
                            None
                        } else {
                            Some(kind)
                        }
                    );
                    for (i, partial) in [0.0, 0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
                        let actual = hands.swing.progress(partial);
                        let expected = row[2 + i].as_f64().unwrap() as f32;
                        assert!(
                            (actual - expected).abs() < 1e-6,
                            "{name} duration={duration} tick={tick} partial={partial}: {actual} != {expected}"
                        );
                    }
                }
                scenario += 1;
            }
        }
        assert_eq!(scenario, custom.len());
        let interact = result["interact"].as_array().unwrap();
        let mut scenario = 0;
        for (name, kind) in [
            ("none", SwingKind::None),
            ("whack", SwingKind::Whack),
            ("stab", SwingKind::Stab),
        ] {
            for duration in [0, 1, 6, 13, 255, i32::MAX] {
                for hand in [Hand::Main, Hand::Off] {
                    let slot = if hand == Hand::Main {
                        PlayerSlot::Storage(0)
                    } else {
                        PlayerSlot::Offhand
                    };
                    let stack = ItemStack::new(
                        "minecraft:stone".into(),
                        1,
                        64,
                        Tag::Compound(vec![
                            (
                                "minecraft:attack_animation".into(),
                                Tag::Compound(vec![("duration".into(), Tag::Int(3))]),
                            ),
                            (
                                "minecraft:interact_animation".into(),
                                Tag::Compound(vec![
                                    ("type".into(), Tag::String(name.into())),
                                    ("duration".into(), Tag::Int(duration)),
                                ]),
                            ),
                        ]),
                    )
                    .unwrap();
                    let mut inventory = PlayerInventory::default();
                    inventory.set_slot(slot, Some(stack));
                    let mut hands = FirstPersonState::default();
                    hands.begin_interaction(&inventory);
                    inventory.set_slot(slot, None);
                    for (tick, row) in interact[scenario].as_array().unwrap().iter().enumerate() {
                        let started = if tick % 3 == 0 {
                            i64::from(hands.start_swing(
                                hand,
                                &Default::default(),
                                &Default::default(),
                            ))
                        } else {
                            -1
                        };
                        hands.swing.tick();
                        assert_eq!(
                            started,
                            row[0].as_i64().unwrap(),
                            "{name} duration={duration} hand={hand:?} tick={tick}"
                        );
                        assert_eq!(
                            hands.swing.kind(),
                            if row[1].as_str().unwrap().is_empty() {
                                None
                            } else {
                                Some(kind)
                            }
                        );
                        for (i, partial) in [0.0, 0.25, 0.5, 0.75, 1.0].into_iter().enumerate() {
                            let actual = hands.swing.progress(partial);
                            let expected = row[2 + i].as_f64().unwrap() as f32;
                            assert!(
                                (actual - expected).abs() < 1e-6,
                                "{name} duration={duration} hand={hand:?} tick={tick} partial={partial}: {actual} != {expected}"
                            );
                        }
                    }
                    hands.end_interaction();
                    scenario += 1;
                }
            }
        }
        assert_eq!(scenario, interact.len());
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
        for row in result["swapScales"].as_array().unwrap() {
            let speed = row[0].as_f64().unwrap();
            let ticks = row[1].as_i64().unwrap() as i32;
            let partial = row[2].as_f64().unwrap() as f32;
            let actual =
                crate::combat::AttackCharge::for_swap_test(ticks, speed).item_swap_scale(partial);
            let expected = row[3].as_f64().unwrap() as f32;
            assert_eq!(
                actual, expected,
                "speed={speed} ticks={ticks} partial={partial}"
            );
        }
        let mut inventory = PlayerInventory::default();
        let mut hands = FirstPersonState::default();
        for (tick, row) in result["scaledEquipment"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            match tick {
                0 => {
                    inventory.set_slot(PlayerSlot::Storage(0), Some(stack("minecraft:stone", 4)));
                }
                12 => {
                    inventory.set_slot(PlayerSlot::Storage(0), Some(stack("minecraft:dirt", 5)));
                }
                15 => {
                    inventory.set_slot(PlayerSlot::Offhand, Some(stack("minecraft:stone", 4)));
                }
                _ => {}
            }
            hands.tick_with_swap(&inventory, row[0].as_f64().unwrap() as f32);
            for (hand, offset) in [(&hands.main, 1), (&hands.off, 5)] {
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
                    let expected = row[index].as_f64().unwrap() as f32;
                    assert!(
                        (actual - expected).abs() < 1e-6,
                        "tick={tick} {actual} != {expected}"
                    );
                }
            }
        }
    }
}
