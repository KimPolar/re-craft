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
    for (float seconds : new float[]{0.05f, 0.25f, 0.81f, 1.6f, 12.75f}) {
      var json = new JsonObject(); json.addProperty("consume_seconds", seconds);
      var consumable = Consumable.CODEC.parse(JsonOps.INSTANCE, json).getOrThrow();
      var row = new JsonObject(); row.addProperty("seconds", seconds); row.addProperty("ticks", consumable.consumeTicks()); times.add(row);
    }
    var out = new JsonObject(); out.add("food", rows); out.add("times", times);
    Files.writeString(Path.of(args[0]), new Gson().toJson(out));
  }
}
