import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.ItemStackTemplate;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.component.BundleContents;

public class BundleDestroyedOracle {
    static class World extends ServerLevel {
        List<ItemEntity> spawned;
        World() { super(null, null, null, null, null, null, false, 0L, null, false); }
        @Override public int getNextEntityId() { return 7; }
        @Override public boolean addFreshEntity(Entity entity) {
            spawned.add((ItemEntity) entity); return true;
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registry = VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registry)) pending.apply();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world = (World) ((sun.misc.Unsafe) field.get(null)).allocateInstance(World.class);
        world.spawned = new ArrayList<>();
        var stack = new ItemStack(Items.BUNDLE);
        stack.set(DataComponents.BUNDLE_CONTENTS, new BundleContents(List.of(
            new ItemStackTemplate(Items.STONE, 3), new ItemStackTemplate(Items.DIRT, 2))));
        var parent = new ItemEntity(world, 3.5, 5.8, 4.5, stack);
        stack.onDestroyed(parent);
        var result = new JsonArray();
        for (var item : world.spawned) {
            var row = new JsonObject();
            row.addProperty("id", BuiltInRegistries.ITEM.getKey(item.getItem().getItem()).toString());
            row.addProperty("count", item.getItem().getCount());
            row.addProperty("x", item.getX()); row.addProperty("y", item.getY()); row.addProperty("z", item.getZ());
            row.addProperty("pickup_delay", item.hasPickUpDelay()); result.add(row);
        }
        Files.writeString(Path.of(args[0]), result.toString());
    }
}
