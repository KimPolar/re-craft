import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;

public class PoiTypesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        JsonArray rows = new JsonArray();
        for (var block : BuiltInRegistries.BLOCK) {
            String name = BuiltInRegistries.BLOCK.getKey(block).toString();
            for (var state : block.getStateDefinition().getPossibleStates()) {
                JsonObject row = new JsonObject();
                row.addProperty("name", name);
                JsonObject properties = new JsonObject();
                state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                row.add("properties", properties);
                var poi = PoiTypes.forState(state);
                if (poi.isPresent()) {
                    row.addProperty("type", poi.get().getRegisteredName());
                    row.addProperty("tickets", poi.get().value().maxTickets());
                    row.addProperty("range", poi.get().value().validRange());
                }
                rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
    }
}
