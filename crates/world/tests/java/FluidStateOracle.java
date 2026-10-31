import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.SharedConstants;

public class FluidStateOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var rows=new JsonArray();
        for (var block:BuiltInRegistries.BLOCK) {
            var name=BuiltInRegistries.BLOCK.getKey(block).toString();
            for (var state:block.getStateDefinition().getPossibleStates()) {
                var row=new JsonObject(); row.addProperty("name",name);
                var properties=new JsonObject();
                state.getValues().forEach(entry -> properties.addProperty(entry.property().getName(),entry.valueName()));
                row.add("properties",properties);
                row.addProperty("fluid",BuiltInRegistries.FLUID.getKey(state.getFluidState().getType()).toString());
                rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
