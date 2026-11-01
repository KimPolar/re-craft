import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.item.*;
import net.minecraft.world.phys.Vec3;

public class MilkOracle {
  static class Player extends FoodEffectsOracle.TestPlayer {
    boolean cleared;
    @Override public boolean removeAllEffects() { cleared=true; return true; }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var lookup=VanillaRegistries.createWorldLookup(); var registries=VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
    var world=(FoodEffectsOracle.World)unsafe.allocateInstance(FoodEffectsOracle.World.class);
    var player=(Player)unsafe.allocateInstance(Player.class);
    player.world=world; var position=Entity.class.getDeclaredField("position"); position.setAccessible(true); position.set(player,Vec3.ZERO);
    var rows=new JsonArray();
    for(int seed=0;seed<32;seed++) {
      var stack=new ItemStack(Items.MILK_BUCKET); var consumable=stack.get(DataComponents.CONSUMABLE);
      player.rng=RandomSource.create(seed); player.food=new FoodData(); player.food.setFoodLevel(20); player.cleared=false; world.sounds=new JsonArray();
      for(int remaining=consumable.consumeTicks();remaining>0;remaining--)
        if(consumable.shouldEmitParticlesAndSounds(remaining)) consumable.emitParticlesAndSounds(player.rng,player,stack,5);
      consumable.emitParticlesAndSounds(player.rng,player,stack,16);
      for(var effect:consumable.onConsumeEffects()) effect.apply(world,stack,player);
      var returned=stack.get(DataComponents.USE_REMAINDER).convertIntoRemainder(ItemStack.EMPTY,1,false,ignored -> {});
      var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("ticks",consumable.consumeTicks());
      row.addProperty("particles",consumable.hasConsumeParticles()); row.addProperty("animation",consumable.animation().toString());
      row.addProperty("food",stack.has(DataComponents.FOOD)); row.addProperty("can_consume",consumable.canConsume(player,stack));
      row.addProperty("cleared",player.cleared); row.addProperty("remainder",BuiltInRegistries.ITEM.getKey(returned.getItem()).toString());
      row.addProperty("next_bits",Float.floatToRawIntBits(player.rng.nextFloat())); row.add("sounds",world.sounds); rows.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
  }
}
