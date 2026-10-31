import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.sounds.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.item.*;
import net.minecraft.world.level.Level;
import net.minecraft.world.phys.Vec3;

public class FoodEffectsOracle {
  static class World extends ClientLevel {
    JsonArray sounds;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    void capture(SoundEvent sound,SoundSource category,float volume,float pitch) {
      var row=new JsonObject(); row.addProperty("event",sound.location().toString()); row.addProperty("category",category.getName());
      row.addProperty("volume_bits",Float.floatToRawIntBits(volume)); row.addProperty("pitch_bits",Float.floatToRawIntBits(pitch)); sounds.add(row);
    }
    @Override public void playSound(Entity source,double x,double y,double z,SoundEvent sound,SoundSource category,float volume,float pitch) { capture(sound,category,volume,pitch); }
  }
  static class TestPlayer extends Player {
    World world; RandomSource rng; FoodData food; MobEffectInstance applied; boolean poison;
    TestPlayer() { super(null,null); }
    @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
    @Override public Level level() { return world; }
    @Override public RandomSource getRandom() { return rng; }
    @Override public FoodData getFoodData() { return food; }
    @Override public void playSound(SoundEvent sound,float volume,float pitch) { world.capture(sound,SoundSource.PLAYERS,volume,pitch); }
    @Override public void spawnItemParticles(ItemStack stack,int count) { for(int i=0;i<count*4;i++) rng.nextFloat(); }
    @Override public boolean addEffect(MobEffectInstance effect,Entity source) { applied=effect; return true; }
    @Override public boolean removeEffect(Holder<MobEffect> effect) {
      if(effect==net.minecraft.world.effect.MobEffects.POISON && poison) { poison=false; return true; }
      return false;
    }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var lookup=VanillaRegistries.createWorldLookup(); var registries=VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
    player.world=world; var position=Entity.class.getDeclaredField("position"); position.setAccessible(true); position.set(player,Vec3.ZERO);
    var out=new JsonArray();
    for(var item:new Item[]{Items.POISONOUS_POTATO,Items.SPIDER_EYE,Items.CHICKEN,Items.HONEY_BOTTLE}) for(int seed=0;seed<100;seed++) {
      var stack=new ItemStack(item); var food=stack.get(DataComponents.FOOD); var consumable=stack.get(DataComponents.CONSUMABLE);
      player.rng=RandomSource.create(seed); player.food=new FoodData(); player.food.setFoodLevel(10); player.food.setSaturation(0); player.applied=null; player.poison=true; world.sounds=new JsonArray();
      for(int remaining=consumable.consumeTicks();remaining>0;remaining--)
        if(consumable.shouldEmitParticlesAndSounds(remaining)) consumable.emitParticlesAndSounds(player.rng,player,stack,5);
      consumable.emitParticlesAndSounds(player.rng,player,stack,16); food.onConsume(world,player,stack,consumable);
      for(var effect:consumable.onConsumeEffects()) effect.apply(world,stack,player);
      var row=new JsonObject(); row.addProperty("item",item==Items.POISONOUS_POTATO?"poisonous_potato":item==Items.SPIDER_EYE?"spider_eye":item==Items.CHICKEN?"chicken":"honey_bottle");
      row.addProperty("max_stack",stack.getMaxStackSize()); row.addProperty("ticks",consumable.consumeTicks());
      row.addProperty("always",food.canAlwaysEat()); row.addProperty("particles",consumable.hasConsumeParticles());
      row.addProperty("poison_removed",!player.poison);
      var remainder=stack.get(DataComponents.USE_REMAINDER);
      if(remainder!=null) {
        var returned=remainder.convertIntoRemainder(ItemStack.EMPTY,1,false,ignored -> {});
        row.addProperty("remainder",BuiltInRegistries.ITEM.getKey(returned.getItem()).toString());
      }
      row.addProperty("seed",seed); row.addProperty("food",player.food.getFoodLevel());
      row.addProperty("saturation_bits",Float.floatToRawIntBits(player.food.getSaturationLevel()));
      row.addProperty("applied",player.applied!=null);
      if(player.applied!=null) { row.addProperty("duration",player.applied.getDuration()); row.addProperty("amplifier",player.applied.getAmplifier());
        row.addProperty("effect",player.applied.getEffect()==net.minecraft.world.effect.MobEffects.HUNGER?"hunger":"poison"); }
      row.addProperty("next_bits",Float.floatToRawIntBits(player.rng.nextFloat())); out.add(row);
      row.add("sounds",world.sounds);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}
