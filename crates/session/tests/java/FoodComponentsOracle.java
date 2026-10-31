import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.food.FoodProperties;
import net.minecraft.world.item.component.Consumable;

public class FoodComponentsOracle {
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion();
    net.minecraft.server.Bootstrap.bootStrap();
    var rows = new JsonArray();
    for (int nutrition : new int[]{0, 1, 7, 100}) {
      for (float saturation : new float[]{-2.5f, 0.0f, 3.5f, 99.0f}) {
        for (boolean always : new boolean[]{false, true}) {
          var json = new JsonObject();
          json.addProperty("nutrition", nutrition); json.addProperty("saturation", saturation);
          json.addProperty("can_always_eat", always);
          var food = FoodProperties.DIRECT_CODEC.parse(JsonOps.INSTANCE, json).getOrThrow();
          var data = new FoodData(); data.setFoodLevel(13); data.setSaturation(1.25f); data.eat(food);
          var row = new JsonObject(); row.addProperty("nutrition", food.nutrition());
          row.addProperty("saturation", food.saturation()); row.addProperty("always", food.canAlwaysEat());
          row.addProperty("result_food", data.getFoodLevel()); row.addProperty("result_saturation", data.getSaturationLevel()); rows.add(row);
        }
      }
    }
    var times = new JsonArray();
    for (float seconds : new float[]{0.0f, 0.01f, 0.049f, 0.05f, 0.25f, 0.81f, 1.6f, 12.75f, 12.8f, 15.0f, 1000000000.0f}) {
      var json = new JsonObject(); json.addProperty("consume_seconds", seconds);
      var consumable = Consumable.CODEC.parse(JsonOps.INSTANCE, json).getOrThrow();
      var row = new JsonObject(); row.addProperty("seconds", seconds); row.addProperty("ticks", consumable.consumeTicks()); times.add(row);
    }
    var remainders = new JsonArray();
    net.minecraft.world.item.Items.BOWL.builtInRegistryHolder().bindComponents(
      net.minecraft.core.component.DataComponentMap.builder().set(net.minecraft.core.component.DataComponents.MAX_STACK_SIZE, 64).build());
    net.minecraft.world.item.Items.APPLE.builtInRegistryHolder().bindComponents(
      net.minecraft.core.component.DataComponentMap.builder().set(net.minecraft.core.component.DataComponents.MAX_STACK_SIZE, 64).build());
    var expanded = new JsonObject(); expanded.addProperty("id", "minecraft:bowl"); expanded.addProperty("count", 3);
    for (var value : new JsonElement[]{new JsonPrimitive("minecraft:bowl"), expanded}) {
    var template = net.minecraft.world.item.component.UseRemainder.CODEC.parse(JsonOps.INSTANCE, value).getOrThrow();
    for (int before : new int[]{1, 2, 20}) for (boolean creative : new boolean[]{false, true}) {
      int after = creative ? before : before - 1;
      var stack = after == 0 ? net.minecraft.world.item.ItemStack.EMPTY : new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.APPLE, after);
      var extras = new java.util.ArrayList<net.minecraft.world.item.ItemStack>();
      var result = template.convertIntoRemainder(stack, before, creative, extras::add);
      var row = new JsonObject(); row.addProperty("before", before); row.addProperty("creative", creative); row.addProperty("template_count", value.isJsonPrimitive() ? 1 : 3);
      row.addProperty("hand_id", result.isEmpty() ? "minecraft:air" : net.minecraft.core.registries.BuiltInRegistries.ITEM.getKey(result.getItem()).toString());
      row.addProperty("hand_count", result.getCount()); row.addProperty("extra_count", extras.isEmpty() ? 0 : extras.getFirst().getCount()); remainders.add(row);
    }
    }
    var out = new JsonObject(); out.add("food", rows); out.add("times", times); out.add("remainders", remainders);
    Files.writeString(Path.of(args[0]), new Gson().toJson(out));
  }
}
