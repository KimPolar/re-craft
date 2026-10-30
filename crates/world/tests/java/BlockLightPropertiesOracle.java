import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.block.RedStoneOreBlock;

public class BlockLightPropertiesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var item : cases) {
            var entry = item.getAsJsonObject();
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(entry.get("name").getAsString()));
            var state = block.defaultBlockState();
            if (entry.has("lit")) state = state.setValue(RedStoneOreBlock.LIT, entry.get("lit").getAsBoolean());
            var row = new JsonArray();
            row.add(state.getLightDampening());
            row.add(state.getLightEmission());
            row.add(state.useShapeForLightOcclusion());
            output.add(row);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
