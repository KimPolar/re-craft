import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;

public class PickFenceDoorOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = net.minecraft.data.registries.VanillaRegistries.createWorldLookup();
        var reloadable = net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) pending.apply();
        var rows = new JsonArray();
        for (int i = 1; i < args.length; i++) {
            var key = Identifier.parse(args[i]);
            if (!BuiltInRegistries.BLOCK.containsKey(key)) throw new AssertionError("unknown block " + key);
            var block = BuiltInRegistries.BLOCK.getValue(key);
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var stack = state.getCloneItemStack(null, BlockPos.ZERO, false);
                var row = new JsonObject();
                row.addProperty("block", key.toString());
                row.addProperty("item", BuiltInRegistries.ITEM.getKey(stack.getItem()).toString());
                row.addProperty("count", stack.getCount());
                row.addProperty("max", stack.getMaxStackSize());
                var properties = new JsonObject();
                state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                row.add("properties", properties);
                rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
