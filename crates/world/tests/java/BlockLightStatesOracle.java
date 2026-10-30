import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;

public class BlockLightStatesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var names = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var result = new JsonArray();
        for (var name : names) {
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(name.getAsString()));
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var entry = new JsonObject();
                var properties = new JsonObject();
                state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                entry.addProperty("name", name.getAsString());
                entry.add("properties", properties);
                entry.addProperty("opacity", state.getLightDampening());
                entry.addProperty("emission", state.getLightEmission());
                entry.addProperty("shaped", state.useShapeForLightOcclusion());
                result.add(entry);
            }
        }
        Files.writeString(Path.of(args[1]), result.toString());
    }
}
