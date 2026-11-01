import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.Unit;
import net.minecraft.world.item.*;

public class ItemDurabilityBarOracle {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var world = VanillaRegistries.createWorldLookup();
        var lookup = VanillaRegistries.createReloadableLookup(world);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(lookup)) pending.apply();
        var rows = new JsonArray();
        for (String name : new String[]{"iron_sword", "iron_boots", "stone"})
            for (int maximum : new int[]{0, 5, 100})
                for (int kind = 0; kind < 4; kind++)
                    for (int damage : new int[]{0, 1, 2, 4, 20, 250}) {
                        var item = switch (name) { case "iron_sword" -> Items.IRON_SWORD; case "iron_boots" -> Items.IRON_BOOTS; default -> Items.STONE; };
                        var stack = new ItemStack(item);
                        if (maximum > 0) stack.set(DataComponents.MAX_DAMAGE, maximum);
                        stack.set(DataComponents.DAMAGE, damage);
                        if (kind == 1) stack.set(DataComponents.UNBREAKABLE, Unit.INSTANCE);
                        if (kind == 2) stack.remove(DataComponents.MAX_DAMAGE);
                        if (kind == 3) stack.remove(DataComponents.DAMAGE);
                        var row = new JsonObject(); row.addProperty("name", name); row.addProperty("maximum", maximum);
                        row.addProperty("kind", kind); row.addProperty("damage", damage);
                        row.addProperty("visible", stack.isBarVisible());
                        if (stack.isBarVisible()) { row.addProperty("width", stack.getBarWidth()); row.addProperty("color", stack.getBarColor()); }
                        rows.add(row);
                    }
        Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
    }
}
