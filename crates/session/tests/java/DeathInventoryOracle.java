import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.util.Prediction;
import net.minecraft.world.entity.EntityEquipment;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.inventory.InventoryMenu;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.enchantment.EnchantmentHelper;
import net.minecraft.world.item.enchantment.EnchantmentEffectComponents;
import net.minecraft.world.item.enchantment.Enchantments;
import net.minecraft.world.item.enchantment.ItemEnchantments;

/** Calls the installed game's inventory and container implementations. */
public class DeathInventoryOracle {
    static Item[] ITEMS;
    static net.minecraft.core.HolderLookup.Provider world;
    static class TestLevel extends ServerLevel {
        TestLevel() { super(null,null,null,null,null,null,false,0,null,false); }
        @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.VANILLA_SET; }
        @Override public boolean isClientSide() { return false; }
    }
    static class TestInventory extends Inventory {
        TestInventory(TestPlayer player,EntityEquipment equipment) { super(player,equipment); }
        // Avoid mock network packets while retaining the actual insertion algorithm.
        @Override public void placeItemBackInInventory(ItemStack item,Prediction prediction) {
            super.placeItemBackInInventory(item,false,prediction);
        }
    }
    static class TestPlayer extends ServerPlayer {
        TestInventory inventory; TestLevel fakeLevel; List<ItemStack> drops,overflow;
        boolean expectedTrace;
        TestPlayer() { super(null,null,null,null); }
        @Override public Inventory getInventory() { return inventory; }
        @Override public ServerLevel level() { return fakeLevel; }
        @Override public boolean hasDisconnected() { return false; }
        @Override public boolean hasInfiniteMaterials() { return false; }
        @Override public void onEquipItem(EquipmentSlot slot,ItemStack before,ItemStack after) {}
        void removeVanishing() { destroyVanishingCursedItems(); }
        @Override public ItemEntity createItemStackToDrop(ItemStack stack,boolean scatter,boolean trace) {
            if (!stack.isEmpty()) { if (!scatter || trace) throw new AssertionError(); drops.add(stack.copy()); }
            return null;
        }
        @Override public ItemEntity drop(ItemStack stack,boolean trace,Prediction prediction) {
            if (trace != expectedTrace) throw new AssertionError("unexpected thrower attribution");
            if (!stack.isEmpty()) overflow.add(stack.copy());
            return null;
        }
    }
    static ItemStack stack(JsonArray spec) {
        int id=spec.get(0).getAsInt(), count=spec.get(1).getAsInt();
        if (id==0) return ItemStack.EMPTY;
        var result=new ItemStack(ITEMS[id],count);
        if (spec.size()>2 && spec.get(2).getAsInt()==1) {
            result.set(DataComponents.ENCHANTMENTS, ItemEnchantments.EMPTY);
            result.enchant(world.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(Enchantments.VANISHING_CURSE),1);
        }
        return result;
    }
    static JsonArray describe(ItemStack stack) {
        JsonArray result=new JsonArray();
        if (stack.isEmpty()) { result.add(0); result.add(0); result.add(0); return result; }
        for (int i=1;i<ITEMS.length;i++) if (stack.is(ITEMS[i])) {
            result.add(i); result.add(stack.getCount());
            result.add(EnchantmentHelper.has(stack,EnchantmentEffectComponents.PREVENT_EQUIPMENT_DROP) ? 1 : 0);
            return result;
        }
        throw new AssertionError(BuiltInRegistries.ITEM.getKey(stack.getItem()));
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        world=VanillaRegistries.createWorldLookup();
        var reloadable=VanillaRegistries.createReloadableLookup(world);
        for (var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) pending.apply();
        ITEMS=new Item[]{Items.AIR,Items.STONE,Items.DIRT,Items.APPLE,
            Items.OAK_PLANKS,Items.COBBLESTONE,Items.GRASS_BLOCK,Items.IRON_INGOT,Items.GOLD_INGOT};
        for (int i=1;i<ITEMS.length;i++) ITEMS[i].builtInRegistryHolder().bindComponents(
            DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
        Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        TestPlayer player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        player.fakeLevel=(TestLevel)unsafe.allocateInstance(TestLevel.class);
        Field playerInventory=net.minecraft.world.entity.player.Player.class.getDeclaredField("inventory");
        playerInventory.setAccessible(true);
        JsonArray cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results=new JsonArray();
        for (var value:cases) {
            var spec=value.getAsJsonObject();
            player.inventory=new TestInventory(player,new EntityEquipment());
            playerInventory.set(player,player.inventory);
            player.inventory.setSelectedSlot(spec.get("selected").getAsInt());
            player.drops=new ArrayList<>(); player.overflow=new ArrayList<>();
            player.expectedTrace=false;
            for (int i=0;i<36;i++) player.inventory.setItem(i,stack(spec.getAsJsonArray("slots").get(i).getAsJsonArray()));
            var armor=spec.getAsJsonArray("armor");
            for (int i=0;i<4;i++) player.inventory.setItem(39-i,stack(armor.get(i).getAsJsonArray()));
            player.inventory.setItem(40,stack(spec.getAsJsonArray("offhand")));
            var menu=new InventoryMenu(player.inventory,false,player);
            Object grid=menu.getSlot(1).container;
            Field gridItems=grid.getClass().getDeclaredField("items"); gridItems.setAccessible(true);
            @SuppressWarnings("unchecked") var items=(List<ItemStack>)gridItems.get(grid);
            for (int i=0;i<4;i++) items.set(i,stack(spec.getAsJsonArray("craft").get(i).getAsJsonArray()));
            menu.setCarried(stack(spec.getAsJsonArray("cursor")));
            if (!spec.get("keep").getAsBoolean()) { player.removeVanishing(); player.inventory.dropAll(); }
            menu.removed(player);
            JsonObject row=new JsonObject(); JsonArray loot=new JsonArray(), overflow=new JsonArray(), slots=new JsonArray(), equipment=new JsonArray();
            for (var stack:player.drops) loot.add(describe(stack));
            for (var stack:player.overflow) overflow.add(describe(stack));
            for (int i=0;i<36;i++) slots.add(describe(player.inventory.getItem(i)));
            for (int i=0;i<4;i++) equipment.add(describe(player.inventory.getItem(39-i)));
            row.add("loot",loot); row.add("overflow",overflow); row.add("slots",slots);
            row.add("armor",equipment); row.add("offhand",describe(player.inventory.getItem(40)));
            results.add(row);
        }
        var menu = new InventoryMenu(player.inventory,false,player);
        player.expectedTrace=true;
        player.overflow.clear();
        menu.setCarried(new ItemStack(Items.DIRT, 2));
        menu.clicked(-999,0,net.minecraft.world.inventory.ContainerInput.PICKUP,player);
        if (player.overflow.size() != 1 || player.overflow.get(0).getCount() != 2)
            throw new AssertionError("outside inventory click did not drop the carried stack");
        for (int i=0;i<36;i++) player.inventory.setItem(i,new ItemStack(Items.STONE,64));
        player.inventory.setItem(0,new ItemStack(Items.STONE,70));
        player.inventory.setItem(10,new ItemStack(Items.DIRT,50));
        player.overflow.clear();
        menu.clicked(10,0,net.minecraft.world.inventory.ContainerInput.SWAP,player);
        if (player.overflow.size() != 1 || player.overflow.get(0).getCount() != 50 || !player.overflow.get(0).is(Items.DIRT))
            throw new AssertionError("inventory shortcut did not drop the displaced stack");
        Files.writeString(Path.of(args[1]),new Gson().toJson(results));
    }
}
