import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.SimpleContainer;
import net.minecraft.world.Container;
import net.minecraft.world.entity.EntityEquipment;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.inventory.*;
import net.minecraft.world.item.*;
import net.minecraft.world.level.GameType;
import net.minecraft.util.Prediction;

public class CraftingRemainderSlotOracle {
    static class TestPlayer extends Player {
        Inventory testInventory;
        List<ItemStack> dropped;
        boolean creative;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean hasInfiniteMaterials() { return creative; }
        @Override public Inventory getInventory() { return testInventory; }
        @Override public ItemEntity drop(ItemStack stack, boolean random, Prediction prediction) {
            dropped.add(stack.copy()); return null;
        }
    }
    static class Menu extends AbstractContainerMenu {
        Menu() { super(null, 0); }
        @Override public ItemStack quickMoveStack(Player player, int index) { return ItemStack.EMPTY; }
        @Override public boolean stillValid(Player player) { return true; }
        @Override public void slotsChanged(Container container) { }
    }
    static String describe(ItemStack stack) {
        return stack.isEmpty() ? "" : BuiltInRegistries.ITEM.getKey(stack.getItem()) + ":" + stack.getCount();
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var world = VanillaRegistries.createWorldLookup();
        var lookup = VanillaRegistries.createReloadableLookup(world);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(lookup)) pending.apply();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var rows = new JsonArray();
        for (int count : new int[]{1, 2, 16}) for (int storage : new int[]{0, 1, 2}) for (boolean creative : new boolean[]{false, true}) {
            var player = (TestPlayer)unsafe.allocateInstance(TestPlayer.class);
            player.testInventory = new Inventory(player, new EntityEquipment()); player.dropped = new ArrayList<>(); player.creative = creative;
            if (storage != 0) for (int i = 0; i < 36; i++) player.testInventory.setItem(i, new ItemStack(Items.DIRT, 64));
            if (storage == 1) player.testInventory.setItem(0, new ItemStack(Items.GLASS_BOTTLE, 63));
            var menu = new Menu(); var grid = new TransientCraftingContainer(menu, 2, 2);
            for (int i = 0; i < 4; i++) grid.setItem(i, new ItemStack(Items.HONEY_BOTTLE, count));
            var slot = new ResultSlot(player, grid, new SimpleContainer(1), 0, 0, 0);
            slot.onTake(player, new ItemStack(Items.HONEY_BLOCK));
            var row = new JsonObject(); row.addProperty("count", count); row.addProperty("storage", storage); row.addProperty("creative", creative);
            var cells = new JsonArray(); for (int i = 0; i < 4; i++) cells.add(describe(grid.getItem(i))); row.add("grid", cells);
            var slots = new JsonArray(); for (int i = 0; i < 36; i++) slots.add(describe(player.testInventory.getItem(i))); row.add("slots", slots);
            var drops = new JsonArray(); for (var item : player.dropped) drops.add(describe(item)); row.add("drops", drops);
            rows.add(row);
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
    }
}
