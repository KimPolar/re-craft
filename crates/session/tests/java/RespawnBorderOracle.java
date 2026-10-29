import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.phys.AABB;

public class RespawnBorderOracle {
    public static void main(String[] args) throws Exception {
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (JsonElement item : cases) {
            JsonObject c = item.getAsJsonObject();
            WorldBorder.Settings settings = new WorldBorder.Settings(c.get("x").getAsDouble(), c.get("z").getAsDouble(),
                0.2, 5, 5, 300, c.get("size").getAsDouble(), c.get("duration").getAsLong(), c.get("target").getAsDouble());
            WorldBorder border = new WorldBorder(settings);
            border.applyInitialSettings(1000);
            for (int i=0; i<c.get("ticks").getAsInt(); i++) border.tick();
            JsonArray v = c.getAsJsonArray("box");
            AABB box = new AABB(v.get(0).getAsDouble(),v.get(1).getAsDouble(),v.get(2).getAsDouble(),
                v.get(3).getAsDouble(),v.get(4).getAsDouble(),v.get(5).getAsDouble());
            JsonArray out = new JsonArray();
            out.add(border.getMinX()); out.add(border.getMaxX()); out.add(border.getMinZ()); out.add(border.getMaxZ());
            out.add(border.isWithinBounds(box)); results.add(out);
        }
        Files.writeString(Path.of(args[1]),new Gson().toJson(results));
    }
}
