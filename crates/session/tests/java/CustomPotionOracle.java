import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.item.*;
import net.minecraft.world.item.alchemy.PotionContents;
import net.minecraft.world.item.alchemy.Potions;
import net.minecraft.world.phys.Vec3;

public class CustomPotionOracle {
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var lookup=VanillaRegistries.createWorldLookup(); var registries=VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var ops=registries.createSerializationContext(JsonOps.INSTANCE);
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
    var world=(FoodEffectsOracle.World)unsafe.allocateInstance(FoodEffectsOracle.World.class);
    var player=(FoodEffectsOracle.TestPlayer)unsafe.allocateInstance(FoodEffectsOracle.TestPlayer.class);
    player.world=world; var position=Entity.class.getDeclaredField("position"); position.setAccessible(true); position.set(player,Vec3.ZERO);
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var out=new JsonArray();
    for(var value:cases) {
      var input=value.getAsJsonObject();
      var contents=PotionContents.CODEC.parse(ops,input.get("contents")).getOrThrow();
      float scale=input.get("scale").getAsFloat(); int seed=input.get("seed").getAsInt();
      var stack=new ItemStack(Items.POTION); stack.set(DataComponents.POTION_CONTENTS,contents); stack.set(DataComponents.POTION_DURATION_SCALE,scale);
      var consumable=stack.get(DataComponents.CONSUMABLE); var remainder=stack.get(DataComponents.USE_REMAINDER);
      player.rng=RandomSource.create(seed); player.food=new FoodData(); player.food.setFoodLevel(20); player.food.setSaturation(5); world.sounds=new JsonArray();
      for(int remaining=consumable.consumeTicks();remaining>0;remaining--)
        if(consumable.shouldEmitParticlesAndSounds(remaining)) consumable.emitParticlesAndSounds(player.rng,player,stack,5);
      consumable.emitParticlesAndSounds(player.rng,player,stack,16);
      var effects=new JsonArray();
      contents.forEachEffect(effect -> { var row=new JsonObject(); row.addProperty("id",effect.getEffect().getRegisteredName());
        row.addProperty("duration",effect.getDuration()); row.addProperty("amplifier",effect.getAmplifier()); row.addProperty("ambient",effect.isAmbient());
        row.addProperty("visible",effect.isVisible()); row.addProperty("icon",effect.showIcon()); effects.add(row); },scale);
      var returned=remainder.convertIntoRemainder(ItemStack.EMPTY,1,false,ignored -> {});
      var row=new JsonObject(); row.addProperty("color",contents.getColor() & 0xffffff); row.addProperty("water",contents.is(Potions.WATER));
      row.addProperty("can_consume",consumable.canConsume(player,stack)); row.addProperty("remainder",BuiltInRegistries.ITEM.getKey(returned.getItem()).toString());
      row.addProperty("next_bits",Float.floatToRawIntBits(player.rng.nextFloat())); row.add("effects",effects); row.add("sounds",world.sounds); out.add(row);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(out));
  }
}
