import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;

public class OrePlacementOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var output = new JsonArray();
        for (int i = 1; i < args.length; i++) {
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(args[i]));
            var state = block.getStateForPlacement(null);
            var row = new JsonObject();
            state.getValues().forEach(entry -> row.addProperty(entry.property().getName(), entry.valueName()));
            output.add(row);
        }
        Files.writeString(Path.of(args[0]), output.toString());
    }
}
