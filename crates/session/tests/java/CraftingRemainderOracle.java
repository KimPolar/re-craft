import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.registries.BuiltInRegistries;

public class CraftingRemainderOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var world = VanillaRegistries.createWorldLookup();
        var lookup = VanillaRegistries.createReloadableLookup(world);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(lookup)) pending.apply();
        var result = new JsonObject();
        for (var item : BuiltInRegistries.ITEM) {
            var template = item.getCraftingRemainder();
            if (template == null) continue;
            var stack = template.create();
            var value = new JsonArray();
            value.add(BuiltInRegistries.ITEM.getKey(stack.getItem()).toString());
            value.add(stack.getCount());
            value.add(item.getDefaultMaxStackSize());
            result.add(BuiltInRegistries.ITEM.getKey(item).toString(), value);
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(result));
    }
}
