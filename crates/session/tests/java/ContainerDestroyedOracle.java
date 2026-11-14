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
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.component.ItemContainerContents;

public class ContainerDestroyedOracle {
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
        var root = new JsonObject();
        var types = new JsonObject();
        for (var item : BuiltInRegistries.ITEM) {
            var method = item.getClass().getMethod("onDestroyed", ItemEntity.class);
            types.addProperty(BuiltInRegistries.ITEM.getKey(item).toString(),
                method.getDeclaringClass() == BlockItem.class);
        }
        root.add("block_destruction", types);
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world = (World) ((sun.misc.Unsafe) field.get(null)).allocateInstance(World.class);
        var scenarios = new JsonObject();
        for (var item : List.of(Items.SHULKER_BOX, Items.REDSTONE, Items.STONE, Items.STICK, Items.BUNDLE)) {
            world.spawned = new ArrayList<>();
            var stack = new ItemStack(item);
            var slots = new ArrayList<ItemStack>();
            for (int slot = 0; slot < 8; ++slot) slots.add(ItemStack.EMPTY);
            slots.set(2, new ItemStack(Items.STONE, 3));
            slots.set(7, new ItemStack(Items.DIRT, 2));
            stack.set(DataComponents.CONTAINER, ItemContainerContents.fromItems(slots));
            stack.onDestroyed(new ItemEntity(world, 3.5, 5.8, 4.5, stack));
            var drops = new JsonArray();
            for (var drop : world.spawned) {
                var row = new JsonObject();
                row.addProperty("id", BuiltInRegistries.ITEM.getKey(drop.getItem().getItem()).toString());
                row.addProperty("count", drop.getItem().getCount());
                row.addProperty("pickup_delay", drop.hasPickUpDelay());
                drops.add(row);
            }
            scenarios.add(BuiltInRegistries.ITEM.getKey(item).toString(), drops);
        }
        root.add("scenarios", scenarios);
        Files.writeString(Path.of(args[0]), root.toString());
    }
}
