import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.sounds.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.item.*;
import net.minecraft.world.level.Level;
import net.minecraft.world.phys.Vec3;

public class RottenFleshOracle {
  static class World extends ClientLevel {
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public void playSound(Entity source,double x,double y,double z,SoundEvent sound,SoundSource category,float volume,float pitch) { }
  }
  static class TestPlayer extends Player {
    World world; RandomSource rng; FoodData food; MobEffectInstance applied;
    TestPlayer() { super(null,null); }
    @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
    @Override public Level level() { return world; }
    @Override public RandomSource getRandom() { return rng; }
    @Override public FoodData getFoodData() { return food; }
    @Override public void playSound(SoundEvent sound,float volume,float pitch) { }
    @Override public void spawnItemParticles(ItemStack stack,int count) {
      for (int i=0;i<count*4;i++) rng.nextFloat();
    }
    @Override public boolean addEffect(MobEffectInstance effect,Entity source) { applied=effect; return true; }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var lookup=VanillaRegistries.createWorldLookup(); var registries=VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
    player.world=world; var position=Entity.class.getDeclaredField("position"); position.setAccessible(true); position.set(player,Vec3.ZERO);
    var stack=new ItemStack(Items.ROTTEN_FLESH); var food=stack.get(DataComponents.FOOD); var consumable=stack.get(DataComponents.CONSUMABLE);
    if (consumable.onConsumeEffects().size()!=1) throw new AssertionError("unexpected rotten flesh effects");
    var out=new JsonArray();
    for(int seed=0;seed<100;seed++) {
      player.rng=RandomSource.create(seed); player.food=new FoodData(); player.applied=null;
      for(int remaining=consumable.consumeTicks();remaining>0;remaining--)
        if(consumable.shouldEmitParticlesAndSounds(remaining)) consumable.emitParticlesAndSounds(player.rng,player,stack,5);
      consumable.emitParticlesAndSounds(player.rng,player,stack,16);
      food.onConsume(world,player,stack,consumable);
      boolean applied=consumable.onConsumeEffects().getFirst().apply(world,stack,player);
      var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("applied",applied);
      if (applied) { row.addProperty("duration",player.applied.getDuration()); row.addProperty("amplifier",player.applied.getAmplifier()); }
      row.addProperty("next_bits",Float.floatToRawIntBits(player.rng.nextFloat())); out.add(row);
    }
    var merges=new JsonArray();
    var hidden=MobEffectInstance.class.getDeclaredField("hiddenEffect"); hidden.setAccessible(true);
    for(int duration:new int[]{1,100,600,1200,-1}) for(int amplifier:new int[]{0,1,3}) {
      var existing=new MobEffectInstance(net.minecraft.world.effect.MobEffects.HUNGER,duration,amplifier);
      existing.update(new MobEffectInstance(net.minecraft.world.effect.MobEffects.HUNGER,600,0));
      var row=new JsonObject(); row.addProperty("initial_duration",duration); row.addProperty("initial_amplifier",amplifier);
      row.addProperty("duration",existing.getDuration()); row.addProperty("amplifier",existing.getAmplifier());
      var child=(MobEffectInstance)hidden.get(existing);
      if(child!=null) { row.addProperty("hidden_duration",child.getDuration()); row.addProperty("hidden_amplifier",child.getAmplifier()); }
      merges.add(row);
    }
    var result=new JsonObject(); result.add("samples",out); result.add("merges",merges);
    Files.writeString(Path.of(args[0]),new Gson().toJson(result));
  }
}
