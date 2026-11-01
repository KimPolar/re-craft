import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.item.*;
import net.minecraft.world.item.alchemy.*;
import net.minecraft.world.phys.Vec3;

public class PotionOracle {
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var lookup=VanillaRegistries.createWorldLookup(); var registries=VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
    var world=(FoodEffectsOracle.World)unsafe.allocateInstance(FoodEffectsOracle.World.class);
    var player=(FoodEffectsOracle.TestPlayer)unsafe.allocateInstance(FoodEffectsOracle.TestPlayer.class);
    player.world=world; var position=Entity.class.getDeclaredField("position"); position.setAccessible(true); position.set(player,Vec3.ZERO);
    Holder<Potion>[] potions=new Holder[]{Potions.WATER,Potions.MUNDANE,Potions.THICK,Potions.AWKWARD,Potions.NIGHT_VISION,Potions.LONG_NIGHT_VISION,Potions.WATER_BREATHING,Potions.LONG_WATER_BREATHING,Potions.POISON,Potions.LONG_POISON,Potions.STRONG_POISON,Potions.REGENERATION,Potions.LONG_REGENERATION,Potions.STRONG_REGENERATION,Potions.SLOWNESS,Potions.LONG_SLOWNESS,Potions.STRONG_SLOWNESS,Potions.SWIFTNESS,Potions.LONG_SWIFTNESS,Potions.STRONG_SWIFTNESS,Potions.LEAPING,Potions.LONG_LEAPING,Potions.STRONG_LEAPING};
    var out=new JsonArray();
    for(var potion:potions) for(int seed=0;seed<20;seed++) {
      var stack=new ItemStack(Items.POTION); var contents=new PotionContents(potion); stack.set(DataComponents.POTION_CONTENTS,contents);
      var consumable=stack.get(DataComponents.CONSUMABLE); var remainder=stack.get(DataComponents.USE_REMAINDER);
      player.rng=RandomSource.create(seed); player.food=new FoodData(); player.food.setFoodLevel(20); player.food.setSaturation(5); world.sounds=new JsonArray();
      for(int remaining=consumable.consumeTicks();remaining>0;remaining--)
        if(consumable.shouldEmitParticlesAndSounds(remaining)) consumable.emitParticlesAndSounds(player.rng,player,stack,5);
      consumable.emitParticlesAndSounds(player.rng,player,stack,16);
      var effects=new JsonArray(); contents.forEachEffect(effect -> { var e=new JsonObject(); e.addProperty("id",effect.getEffect().getRegisteredName()); e.addProperty("duration",effect.getDuration()); e.addProperty("amplifier",effect.getAmplifier()); effects.add(e); },1.0f);
      var returned=remainder.convertIntoRemainder(ItemStack.EMPTY,1,false,ignored -> {});
      var row=new JsonObject(); row.addProperty("potion",potion.getRegisteredName()); row.addProperty("seed",seed);
      row.addProperty("ticks",consumable.consumeTicks()); row.addProperty("particles",consumable.hasConsumeParticles()); row.addProperty("animation",consumable.animation().toString());
      row.addProperty("can_consume",consumable.canConsume(player,stack)); row.addProperty("remainder",BuiltInRegistries.ITEM.getKey(returned.getItem()).toString());
      row.addProperty("next_bits",Float.floatToRawIntBits(player.rng.nextFloat())); row.addProperty("color",contents.getColor()); row.add("effects",effects); row.add("sounds",world.sounds); out.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}
