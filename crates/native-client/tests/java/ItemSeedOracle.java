import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.client.renderer.entity.state.ItemClusterRenderState;
import net.minecraft.world.item.ItemStack;

public class ItemSeedOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var world = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(world);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var names = new String[]{"minecraft:stone", "minecraft:diamond_sword", "minecraft:iron_helmet"};
        var values = new int[]{-5, 0, 1, 100, 2000};
        var limits = new int[]{-1, 7};
        var out = new JsonArray();
        for (String name : names) {
            var item = BuiltInRegistries.ITEM.getValue(net.minecraft.resources.Identifier.parse(name));
            for (int value : values) for (int limit : limits) {
                var stack = new ItemStack(item, 1);
                stack.set(DataComponents.DAMAGE, value);
                if (limit >= 0) stack.set(DataComponents.MAX_DAMAGE, limit);
                var row = new JsonObject();
                row.addProperty("name", name); row.addProperty("damage", value); row.addProperty("limit", limit);
                row.addProperty("seed", ItemClusterRenderState.getSeedForItemStack(stack));
                out.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(out));
    }
}
