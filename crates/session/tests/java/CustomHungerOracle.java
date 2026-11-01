import com.google.gson.*;
import com.mojang.serialization.JsonOps;
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
import net.minecraft.world.effect.MobEffects;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.Consumable;
import net.minecraft.world.level.Level;
import net.minecraft.world.phys.Vec3;

public class CustomHungerOracle {
  static class World extends ClientLevel {
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public void playSound(Entity source,double x,double y,double z,SoundEvent sound,SoundSource category,float volume,float pitch) { }
  }
  static class TestPlayer extends Player {
    World world; RandomSource rng; FoodData food; MobEffectInstance active; MobEffectInstance poison;
    TestPlayer() { super(null,null); }
    @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
    @Override public Level level() { return world; }
    @Override public RandomSource getRandom() { return rng; }
    @Override public FoodData getFoodData() { return food; }
    @Override public void playSound(SoundEvent sound,float volume,float pitch) { }
    @Override public void spawnItemParticles(ItemStack stack,int count) { for(int i=0;i<count*4;i++) rng.nextFloat(); }
    @Override public boolean addEffect(MobEffectInstance effect,Entity source) {
      if(effect.getEffect()==net.minecraft.world.effect.MobEffects.POISON) {
        if(poison==null) poison=new MobEffectInstance(effect); else poison.update(effect); return true;
      }
      if(active==null) active=new MobEffectInstance(effect); else active.update(effect); return true;
    }
    @Override public boolean removeEffect(Holder<MobEffect> effect) {
      if(effect==MobEffects.POISON) { boolean had=poison!=null; poison=null; return had; }
      if(effect==MobEffects.HUNGER) { boolean had=active!=null; active=null; return had; }
      return false;
    }
    @Override public boolean removeAllEffects() { boolean had=active!=null||poison!=null; active=null; poison=null; return had; }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var lookup=VanillaRegistries.createWorldLookup(); var registries=VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var ops=net.minecraft.core.RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY).createSerializationContext(JsonOps.INSTANCE);
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
    player.world=world; var position=Entity.class.getDeclaredField("position"); position.setAccessible(true); position.set(player,Vec3.ZERO);
    var hidden=MobEffectInstance.class.getDeclaredField("hiddenEffect"); hidden.setAccessible(true);
    String[] cases={
      "{\"consume_seconds\":0,\"has_consume_particles\":false,\"on_consume_effects\":[{\"type\":\"minecraft:apply_effects\",\"probability\":1,\"effects\":[{\"id\":\"minecraft:hunger\",\"duration\":20},{\"id\":\"minecraft:hunger\",\"duration\":10,\"amplifier\":2}]},{\"type\":\"minecraft:apply_effects\",\"probability\":0,\"effects\":[{\"id\":\"minecraft:hunger\",\"duration\":900,\"amplifier\":3}]}]}",
      "{\"consume_seconds\":0,\"on_consume_effects\":[{\"type\":\"minecraft:apply_effects\",\"probability\":0.35,\"effects\":[{\"id\":\"minecraft:hunger\",\"duration\":70,\"amplifier\":1}]},{\"type\":\"minecraft:apply_effects\",\"probability\":0.8,\"effects\":[{\"id\":\"minecraft:hunger\",\"duration\":300}]}]}",
      "{\"consume_seconds\":0.25,\"has_consume_particles\":false,\"on_consume_effects\":[{\"type\":\"minecraft:apply_effects\",\"effects\":[{\"id\":\"minecraft:hunger\",\"duration\":-1,\"amplifier\":1,\"ambient\":true,\"show_particles\":false}]}]}",
      "{\"consume_seconds\":0,\"on_consume_effects\":[{\"type\":\"minecraft:apply_effects\",\"probability\":0.35,\"effects\":[{\"id\":\"minecraft:hunger\",\"duration\":70,\"amplifier\":1},{\"id\":\"minecraft:poison\",\"duration\":100}]},{\"type\":\"minecraft:apply_effects\",\"probability\":0.8,\"effects\":[{\"id\":\"minecraft:poison\",\"duration\":20,\"amplifier\":2}]}]}",
      "{\"consume_seconds\":0,\"on_consume_effects\":[{\"type\":\"minecraft:remove_effects\",\"effects\":\"minecraft:poison\"},{\"type\":\"minecraft:apply_effects\",\"probability\":0.35,\"effects\":[{\"id\":\"minecraft:poison\",\"duration\":100}]}]}",
      "{\"consume_seconds\":0,\"on_consume_effects\":[{\"type\":\"minecraft:remove_effects\",\"effects\":[\"minecraft:hunger\",\"minecraft:poison\"]},{\"type\":\"minecraft:apply_effects\",\"probability\":0.8,\"effects\":[{\"id\":\"minecraft:poison\",\"duration\":20,\"amplifier\":2}]}]}",
      "{\"consume_seconds\":0,\"on_consume_effects\":[{\"type\":\"minecraft:remove_effects\",\"effects\":[]}]}",
      "{\"consume_seconds\":0,\"on_consume_effects\":[{\"type\":\"minecraft:clear_all_effects\"},{\"type\":\"minecraft:apply_effects\",\"probability\":0.8,\"effects\":[{\"id\":\"minecraft:poison\",\"duration\":20,\"amplifier\":2}]}]}",
      "{\"consume_seconds\":0,\"on_consume_effects\":[{\"type\":\"minecraft:apply_effects\",\"effects\":[{\"id\":\"minecraft:hunger\",\"duration\":40}]},{\"type\":\"minecraft:clear_all_effects\"}]}"
    };
    var out=new JsonArray();
    for(int index=0;index<cases.length;index++) {
      var consumable=Consumable.CODEC.parse(ops,JsonParser.parseString(cases[index])).getOrThrow();
      for(int seed=0;seed<40;seed++) {
        player.rng=RandomSource.create(seed); player.food=new FoodData();
        player.active=index>=4?new MobEffectInstance(MobEffects.HUNGER,50,0):null;
        player.poison=index>=4?new MobEffectInstance(MobEffects.POISON,200,1):null;
        var stack=new ItemStack(Items.APPLE); var food=stack.get(DataComponents.FOOD);
        for(int remaining=consumable.consumeTicks();remaining>0;remaining--)
          if(consumable.shouldEmitParticlesAndSounds(remaining)) consumable.emitParticlesAndSounds(player.rng,player,stack,5);
        consumable.emitParticlesAndSounds(player.rng,player,stack,16); food.onConsume(world,player,stack,consumable);
        for(var effect:consumable.onConsumeEffects()) effect.apply(world,stack,player);
        var row=new JsonObject(); row.addProperty("case",index); row.addProperty("seed",seed);
        if(player.active!=null) {
          row.addProperty("duration",player.active.getDuration()); row.addProperty("amplifier",player.active.getAmplifier());
          var child=(MobEffectInstance)hidden.get(player.active);
          if(child!=null) { row.addProperty("hidden_duration",child.getDuration()); row.addProperty("hidden_amplifier",child.getAmplifier()); }
        }
        if(player.poison!=null) {
          row.addProperty("poison_duration",player.poison.getDuration()); row.addProperty("poison_amplifier",player.poison.getAmplifier());
          var child=(MobEffectInstance)hidden.get(player.poison);
          if(child!=null) { row.addProperty("poison_hidden_duration",child.getDuration()); row.addProperty("poison_hidden_amplifier",child.getAmplifier()); }
        }
        row.addProperty("next_bits",Float.floatToRawIntBits(player.rng.nextFloat())); out.add(row);
      }
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}
