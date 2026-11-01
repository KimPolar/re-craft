//! Compare ordinary slot pickup against the installed 26.3 container implementation.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_persistence::nbt::Tag;
use native_session::inventory::{
    ClickButton, InventoryPointerEvent, InventoryPointerTarget, ItemStack, PlayerInventory,
    PlayerSlot, SlotRules, pickup,
};
use native_session::{GameMode, GameSession, LocalSession, PlayerCommand};
use native_world::{Block, World};
use serde_json::{Value, json};

const ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.nbt.*;
import net.minecraft.world.SimpleContainer;
import net.minecraft.world.entity.EntityEquipment;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.inventory.AbstractContainerMenu;
import net.minecraft.world.inventory.ContainerInput;
import net.minecraft.world.inventory.InventoryMenu;
import net.minecraft.world.inventory.CraftingMenu;
import net.minecraft.world.inventory.ResultContainer;
import net.minecraft.world.inventory.Slot;
import net.minecraft.world.inventory.TransientCraftingContainer;
import net.minecraft.world.inventory.RecipeBookMenu;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.ItemStackTemplate;
import net.minecraft.world.item.crafting.*;
import net.minecraft.recipebook.ServerPlaceRecipe;
import net.minecraft.world.entity.player.StackedItemContents;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.Level;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.util.Prediction;

public class InventoryOracle {
    static class TestLevel extends ClientLevel {
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.VANILLA_SET; }
    }
    static class TestPlayer extends Player {
        Level testLevel; Inventory testInventory; Abilities testAbilities;
        ItemStack dropped;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public Level level() { return testLevel; }
        @Override public Inventory getInventory() { return testInventory; }
        @Override public boolean hasInfiniteMaterials() { return testAbilities.instabuild; }
        @Override public void onEquipItem(EquipmentSlot slot, ItemStack before, ItemStack after) {}
        @Override public ItemEntity drop(ItemStack stack, boolean trace, Prediction prediction) {
            if (!stack.isEmpty()) dropped = stack.copy();
            return null;
        }
    }
    static class TestSlot extends Slot {
        final int limit; final boolean place, take;
        TestSlot(SimpleContainer inventory, int limit, boolean place, boolean take) {
            super(inventory, 0, 0, 0); this.limit=limit; this.place=place; this.take=take;
        }
        @Override public boolean mayPlace(ItemStack stack) { return place; }
        @Override public boolean mayPickup(Player player) { return take; }
        @Override public int getMaxStackSize() { return limit; }
    }
    static class TestMenu extends AbstractContainerMenu {
        TestMenu(SimpleContainer inventory, int limit, boolean place, boolean take) {
            super(null, 0); addSlot(new TestSlot(inventory, limit, place, take));
        }
        @Override public ItemStack quickMoveStack(Player player, int slot) { return ItemStack.EMPTY; }
        @Override public boolean stillValid(Player player) { return true; }
    }
    static class ResultMenu extends AbstractContainerMenu {
        ResultMenu(ResultContainer inventory) {
            super(null, 0);
            addSlot(new Slot(inventory, 0, 0, 0) {
                @Override public boolean mayPlace(ItemStack stack) { return false; }
            });
        }
        @Override public ItemStack quickMoveStack(Player player, int slot) { return ItemStack.EMPTY; }
        @Override public boolean stillValid(Player player) { return true; }
    }
    static ItemStack stack(JsonArray spec) {
        int id = spec.get(0).getAsInt(), count = spec.get(1).getAsInt();
        if (id == 0) return ItemStack.EMPTY;
        var item = switch (id) { case 1,4,6,7 -> Items.STONE; case 2 -> Items.DIRT; case 3 -> Items.ENDER_PEARL; case 5 -> Items.STONE_BRICKS; case 8 -> Items.DIAMOND_SWORD; case 9 -> Items.APPLE; default -> throw new IllegalArgumentException(); };
        ItemStack value = new ItemStack(item, count);
        if (id == 4) value.set(DataComponents.MAX_STACK_SIZE, 16);
        if (id == 6) value.set(DataComponents.MAX_STACK_SIZE, 99);
        if (id == 7) value.set(DataComponents.MAX_STACK_SIZE, 64);
        if (id == 8) value.set(DataComponents.DAMAGE, 13);
        return value;
    }
    static JsonArray output(ItemStack item) {
        JsonArray result = new JsonArray();
        if (item.isEmpty()) { result.add(0); result.add(0); return result; }
        int id = item.is(Items.DIRT) ? 2 : item.is(Items.ENDER_PEARL) ? 3 : item.is(Items.STONE_BRICKS) ? 5 : item.is(Items.DIAMOND_SWORD) ? 8 : item.is(Items.APPLE) ? 9 : item.getMaxStackSize() == 16 ? 4 : item.getMaxStackSize() == 99 ? 6 : 1;
        result.add(id); result.add(item.getCount()); return result;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        for (var entry : new Object[][]{{Items.STONE,64},{Items.DIRT,64},{Items.ENDER_PEARL,16},{Items.STONE_BRICKS,64},{Items.DIAMOND_SWORD,1},{Items.APPLE,64}}) {
            var item=(net.minecraft.world.item.Item)entry[0]; int max=(Integer)entry[1];
            item.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE, max).build());
        }
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe) field.get(null);
        TestPlayer player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        player.testLevel = (TestLevel) unsafe.allocateInstance(TestLevel.class);
        player.testInventory = new Inventory(player, new EntityEquipment());
        player.testAbilities = new Abilities();
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (var value : cases) {
            JsonObject input = value.getAsJsonObject();
            if (input.has("pick")) {
                var spec=input.getAsJsonObject("pick");
                player.testInventory=new Inventory(player,new EntityEquipment());
                for(int i=0;i<36;i++) player.testInventory.setItem(i,stack(spec.getAsJsonArray("slots").get(i).getAsJsonArray()));
                player.testInventory.setSelectedSlot(spec.get("selected").getAsInt());
                ItemStack item=stack(spec.getAsJsonArray("item"));
                int index=player.testInventory.findSlotMatchingItem(item);
                if (Inventory.isHotbarSlot(index)) player.testInventory.setSelectedSlot(index);
                else if (index != -1) player.testInventory.pickSlot(index);
                else if (spec.get("creative").getAsBoolean()) player.testInventory.addAndPickItem(item);
                JsonArray result=new JsonArray(), slots=new JsonArray();
                for(int i=0;i<36;i++) slots.add(output(player.testInventory.getItem(i)));
                result.add(player.testInventory.getSelectedSlot()); result.add(slots); results.add(result); continue;
            }
            if (input.has("cloneBlock")) {
                var block=net.minecraft.core.registries.BuiltInRegistries.BLOCK.getValue(Identifier.parse(input.get("cloneBlock").getAsString()));
                block.asItem().builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
                JsonArray result=new JsonArray();
                for(var state : block.getStateDefinition().getPossibleStates()) {
                    var item=state.getCloneItemStack(null, net.minecraft.core.BlockPos.ZERO, false);
                    result.add(item.isEmpty() ? "minecraft:air" : net.minecraft.core.registries.BuiltInRegistries.ITEM.getKey(item.getItem()).toString());
                }
                results.add(result); continue;
            }
            if (input.has("drag")) {
                var spec=input.getAsJsonObject("drag"); boolean table=spec.get("table").getAsBoolean();
                player.testAbilities.instabuild=false;
                player.testInventory=new Inventory(player,new EntityEquipment());
                for(int i=0;i<36;i++) player.testInventory.setItem(i,stack(spec.getAsJsonArray("slots").get(i).getAsJsonArray()));
                player.testInventory.setItem(40,stack(spec.getAsJsonArray("offhand")));
                AbstractContainerMenu menu=table ? new CraftingMenu(0,player.testInventory) : new InventoryMenu(player.testInventory,false,player);
                int size=table ? 9 : 4;
                for(int i=0;i<size;i++) menu.getSlot(1+i).set(stack(spec.getAsJsonArray("craft").get(i).getAsJsonArray()));
                menu.setCarried(stack(spec.getAsJsonArray("cursor")));
                int button=spec.get("button").getAsInt();
                menu.clicked(-999,AbstractContainerMenu.getQuickcraftMask(0,button),ContainerInput.QUICK_CRAFT,player);
                for(var valueSlot : spec.getAsJsonArray("visits")) {
                    int target=valueSlot.getAsInt();
                    int index=target==100 ? 45 : target==101 ? 0 : target>=36 ? 1+target-36 : target<9 ? (table ? 37 : 36)+target : (table ? 10+target-9 : target);
                    menu.clicked(index,AbstractContainerMenu.getQuickcraftMask(1,button),ContainerInput.QUICK_CRAFT,player);
                }
                menu.clicked(-999,AbstractContainerMenu.getQuickcraftMask(2,button),ContainerInput.QUICK_CRAFT,player);
                JsonArray result=new JsonArray(), slots=new JsonArray(), craft=new JsonArray();
                for(int i=0;i<36;i++) slots.add(output(player.testInventory.getItem(i)));
                for(int i=0;i<size;i++) craft.add(output(menu.getSlot(1+i).getItem()));
                result.add(slots); result.add(craft); result.add(output(player.testInventory.getItem(40))); result.add(output(menu.getCarried()));
                results.add(result); continue;
            }
            if (input.has("bookPlace")) {
                var spec=input.getAsJsonObject("bookPlace"); int size=spec.get("size").getAsInt();
                player.testInventory=new Inventory(player,new EntityEquipment());
                for(int i=0;i<36;i++) player.testInventory.setItem(i,stack(spec.getAsJsonArray("slots").get(i).getAsJsonArray()));
                var menu=new TestMenu(new SimpleContainer(1),64,true,true);
                var grid=new TransientCraftingContainer(menu,size,size);
                var gridSlots=new java.util.ArrayList<Slot>();
                for(int i=0;i<size*size;i++) { grid.setItem(i,stack(spec.getAsJsonArray("craft").get(i).getAsJsonArray())); gridSlots.add(new Slot(grid,i,0,0)); }
                var pattern=ShapedRecipePattern.of(java.util.Map.of('S',Ingredient.of(Items.STONE)),"SS","SS");
                CraftingRecipe recipe=new ShapedRecipe(new Recipe.CommonInfo(true),new CraftingRecipe.CraftingBookInfo(CraftingBookCategory.BUILDING,""),pattern,new ItemStackTemplate(Items.STONE_BRICKS));
                var key=ResourceKey.create(Registries.RECIPE,Identifier.withDefaultNamespace("stone_bricks"));
                var holder=new RecipeHolder<CraftingRecipe>(key,recipe);
                var access=new ServerPlaceRecipe.CraftingMenuAccess<CraftingRecipe>() {
                    public void fillCraftSlotsStackedContents(StackedItemContents contents) { grid.fillStackedContents(contents); }
                    public void clearCraftingContent() { grid.clearContent(); }
                    public boolean recipeMatches(RecipeHolder<CraftingRecipe> selected) { return selected.value().matches(grid.asCraftInput(),null); }
                };
                var action=ServerPlaceRecipe.placeRecipe(access,size,size,gridSlots,gridSlots,player.testInventory,holder,spec.get("max").getAsBoolean(),false);
                JsonArray result=new JsonArray(), slots=new JsonArray(), craft=new JsonArray();
                for(int i=0;i<36;i++) slots.add(output(player.testInventory.getItem(i)));
                for(int i=0;i<size*size;i++) craft.add(output(grid.getItem(i)));
                result.add(slots); result.add(craft); result.add(action==RecipeBookMenu.PostPlaceAction.PLACE_GHOST_RECIPE); results.add(result); continue;
            }
            if (input.has("tableMove")) {
                var spec=input.getAsJsonObject("tableMove");
                player.testInventory=new Inventory(player,new EntityEquipment());
                for(int i=0;i<36;i++) player.testInventory.setItem(i,stack(spec.getAsJsonArray("slots").get(i).getAsJsonArray()));
                var menu=new CraftingMenu(0,player.testInventory);
                for(int i=0;i<9;i++) menu.getSlot(1+i).set(stack(spec.getAsJsonArray("craft").get(i).getAsJsonArray()));
                int source=spec.get("source").getAsInt();
                int slot=source>=36 ? 1+source-36 : source<9 ? 37+source : 10+source-9;
                menu.clicked(slot,0,ContainerInput.QUICK_MOVE,player);
                JsonArray result=new JsonArray(), slots=new JsonArray(), craft=new JsonArray();
                for(int i=0;i<36;i++) slots.add(output(player.testInventory.getItem(i)));
                for(int i=0;i<9;i++) craft.add(output(menu.getSlot(1+i).getItem()));
                result.add(slots); result.add(craft); results.add(result); continue;
            }
            if (input.has("chestMove")) {
                var spec=input.getAsJsonObject("chestMove");
                player.testInventory=new Inventory(player,new EntityEquipment());
                for(int i=0;i<36;i++) player.testInventory.setItem(i,stack(spec.getAsJsonArray("slots").get(i).getAsJsonArray()));
                int count=spec.getAsJsonArray("chest").size();
                var contents=new SimpleContainer(count);
                for(int i=0;i<count;i++) contents.setItem(i,stack(spec.getAsJsonArray("chest").get(i).getAsJsonArray()));
                var menu=count==27 ? net.minecraft.world.inventory.ChestMenu.threeRows(0,player.testInventory,contents) : net.minecraft.world.inventory.ChestMenu.sixRows(0,player.testInventory,contents);
                int source=spec.get("source").getAsInt();
                int slot=source>=36 ? source-36 : source<9 ? count+27+source : count+source-9;
                menu.clicked(slot,0,ContainerInput.QUICK_MOVE,player);
                JsonArray result=new JsonArray(), slots=new JsonArray(), chest=new JsonArray();
                for(int i=0;i<36;i++) slots.add(output(player.testInventory.getItem(i)));
                for(int i=0;i<count;i++) chest.add(output(contents.getItem(i)));
                result.add(slots); result.add(chest); results.add(result); continue;
            }
            if (input.has("tableResult")) {
                var spec=input.getAsJsonObject("tableResult");
                player.testInventory=new Inventory(player,new EntityEquipment());
                for(int i=0;i<36;i++) player.testInventory.setItem(i,stack(spec.getAsJsonArray("slots").get(i).getAsJsonArray()));
                var menu=new CraftingMenu(0,player.testInventory);
                JsonArray stone=new JsonArray(); stone.add(1); stone.add(1);
                JsonArray resultStack=new JsonArray(); resultStack.add(5); resultStack.add(4);
                for(int i : new int[]{0,1,3,4}) menu.getSlot(1+i).set(stack(stone));
                menu.getResultSlot().set(stack(resultStack));
                player.dropped=ItemStack.EMPTY;
                menu.clicked(0,0,ContainerInput.QUICK_MOVE,player);
                JsonArray result=new JsonArray(), slots=new JsonArray(), craft=new JsonArray();
                for(int i=0;i<36;i++) slots.add(output(player.testInventory.getItem(i)));
                for(int i=0;i<9;i++) craft.add(output(menu.getSlot(1+i).getItem()));
                result.add(slots); result.add(craft); result.add(output(player.dropped)); results.add(result); continue;
            }
            if (input.has("playerResult")) {
                var spec=input.getAsJsonObject("playerResult");
                player.testInventory=new Inventory(player,new EntityEquipment());
                for(int i=0;i<36;i++) player.testInventory.setItem(i,stack(spec.getAsJsonArray("slots").get(i).getAsJsonArray()));
                var menu=new InventoryMenu(player.testInventory,false,player);
                JsonArray stone=new JsonArray(); stone.add(1); stone.add(1);
                JsonArray resultStack=new JsonArray(); resultStack.add(5); resultStack.add(4);
                for(int i=0;i<4;i++) menu.getSlot(1+i).set(stack(stone));
                menu.getResultSlot().set(stack(resultStack));
                player.dropped=ItemStack.EMPTY;
                menu.clicked(0,0,ContainerInput.QUICK_MOVE,player);
                JsonArray result=new JsonArray(), slots=new JsonArray(), craft=new JsonArray();
                for(int i=0;i<36;i++) slots.add(output(player.testInventory.getItem(i)));
                for(int i=0;i<4;i++) craft.add(output(menu.getSlot(1+i).getItem()));
                result.add(slots); result.add(craft); result.add(output(player.dropped)); results.add(result); continue;
            }
            if (input.has("componentCompare")) {
                var spec=input.getAsJsonObject("componentCompare");
                ItemStack first=new ItemStack(Items.STONE,1), second=new ItemStack(Items.STONE,1);
                if(spec.has("first")) first.set(DataComponents.MAX_STACK_SIZE,spec.get("first").getAsInt());
                if(spec.has("second")) second.set(DataComponents.MAX_STACK_SIZE,spec.get("second").getAsInt());
                results.add(ItemStack.isSameItemSameComponents(first,second)); continue;
            }
            if (input.has("swapResult")) {
                var spec=input.getAsJsonObject("swapResult");
                player.testInventory=new Inventory(player,new EntityEquipment());
                int target=spec.get("target").getAsInt();
                player.testInventory.setItem(target,stack(spec.getAsJsonArray("destination")));
                ResultContainer resultSlots=new ResultContainer();
                resultSlots.setItem(0,stack(spec.getAsJsonArray("output")));
                ResultMenu menu=new ResultMenu(resultSlots);
                menu.setCarried(stack(spec.getAsJsonArray("cursor")));
                if(menu.getCarried().isEmpty()) menu.clicked(0,target,ContainerInput.SWAP,player);
                JsonArray result=new JsonArray(); result.add(output(player.testInventory.getItem(target))); result.add(output(menu.getSlot(0).getItem())); result.add(output(menu.getCarried()));
                results.add(result); continue;
            }
            if (input.has("swapCase")) {
                var spec=input.getAsJsonObject("swapCase");
                player.testInventory = new Inventory(player,new EntityEquipment());
                player.testAbilities.instabuild=spec.get("creative").getAsBoolean();
                var values=spec.getAsJsonArray("slots");
                for(int i=0;i<36;i++) player.testInventory.setItem(i,stack(values.get(i).getAsJsonArray()));
                player.testInventory.setItem(40,stack(spec.getAsJsonArray("offhand")));
                player.testInventory.setSelectedSlot(spec.get("selected").getAsInt());
                player.dropped=ItemStack.EMPTY;
                var cursor=stack(spec.getAsJsonArray("cursor"));
                JsonArray crafting=new JsonArray();
                if(spec.get("screen").getAsBoolean()) {
                    var menu=new InventoryMenu(player.testInventory,false,player);
                    var craft=spec.getAsJsonArray("craft");
                    for(int i=0;i<4;i++) menu.getCraftSlots().setItem(i,stack(craft.get(i).getAsJsonArray()));
                    menu.setCarried(cursor);
                    int source=spec.get("source").getAsInt();
                    int menuSlot=source>=37 ? source-36 : source==36 ? 45 : source<9 ? source+36 : source;
                    if(menu.getCarried().isEmpty()) menu.clicked(menuSlot,spec.get("target").getAsInt(),ContainerInput.SWAP,player);
                    cursor=menu.getCarried();
                    for(int i=0;i<4;i++) crafting.add(output(menu.getCraftSlots().getItem(i)));
                } else {
                    int selected=player.testInventory.getSelectedSlot();
                    var off=player.testInventory.getItem(40);
                    player.testInventory.setItem(40,player.testInventory.getItem(selected));
                    player.testInventory.setItem(selected,off);
                    for(int i=0;i<4;i++) crafting.add(output(ItemStack.EMPTY));
                }
                JsonArray slots=new JsonArray(); for(int i=0;i<36;i++) slots.add(output(player.testInventory.getItem(i)));
                JsonArray result=new JsonArray(); result.add(slots); result.add(output(player.testInventory.getItem(40))); result.add(output(cursor)); result.add(output(player.dropped)); result.add(crafting);
                results.add(result); continue;
            }
            if (input.has("dropKind")) {
                String kind = input.get("dropKind").getAsString();
                player.dropped = ItemStack.EMPTY;
                if (kind.equals("selected")) {
                    player.testInventory = new Inventory(player, new EntityEquipment());
                    player.testInventory.setItem(0, stack(input.getAsJsonArray("slot")));
                    player.testInventory.setSelectedSlot(0);
                    ItemStack taken = player.testInventory.removeFromSelected(input.get("all").getAsBoolean());
                    JsonArray result = new JsonArray(); result.add(output(player.testInventory.getItem(0))); result.add(output(taken)); results.add(result);
                } else {
                    SimpleContainer inventory = new SimpleContainer(1);
                    inventory.setItem(0, stack(input.getAsJsonArray("slot")));
                    TestMenu menu = new TestMenu(inventory, 64, true, true);
                    menu.setCarried(stack(input.getAsJsonArray("cursor")));
                    int button = input.get("all").getAsBoolean() ? 1 : 0;
                    if (kind.equals("slot")) menu.clicked(0, button, ContainerInput.THROW, player);
                    else menu.clicked(-999, button, ContainerInput.PICKUP, player);
                    JsonArray result = new JsonArray(); result.add(output(menu.getSlot(0).getItem())); result.add(output(menu.getCarried())); result.add(output(player.dropped)); results.add(result);
                }
                continue;
            }
            if (input.has("returnStack")) {
                player.testInventory = new Inventory(player, new EntityEquipment());
                var source = input.getAsJsonArray("slots");
                for (int i=0;i<36;i++) player.testInventory.setItem(i,stack(source.get(i).getAsJsonArray()));
                player.testInventory.setItem(40, stack(input.getAsJsonArray("offhand")));
                player.testInventory.setSelectedSlot(input.get("selected").getAsInt());
                player.dropped = ItemStack.EMPTY;
                ItemStack item = stack(input.getAsJsonArray("returnStack"));
                player.testInventory.placeItemBackInInventory(item, false, Prediction.SERVER_ONLY);
                JsonArray slots = new JsonArray(); for (int i=0;i<36;i++) slots.add(output(player.testInventory.getItem(i)));
                JsonArray result = new JsonArray(); result.add(slots); result.add(output(player.testInventory.getItem(40))); result.add(output(player.dropped)); results.add(result);
                continue;
            }
            if (input.has("pickup")) {
                player.testInventory = new Inventory(player, new EntityEquipment());
                player.testAbilities.instabuild = input.get("creative").getAsBoolean();
                JsonArray source = input.getAsJsonArray("slots");
                for (int i=0; i<36; i++) player.testInventory.setItem(i, stack(source.get(i).getAsJsonArray()));
                player.testInventory.setItem(40, stack(input.getAsJsonArray("offhand")));
                player.testInventory.setSelectedSlot(input.get("selected").getAsInt());
                ItemStack item = stack(input.getAsJsonArray("pickup"));
                boolean changed = player.testInventory.add(item);
                JsonArray slots = new JsonArray();
                for (int i=0; i<36; i++) slots.add(output(player.testInventory.getItem(i)));
                JsonArray result = new JsonArray();
                result.add(changed); result.add(output(item)); result.add(slots); result.add(output(player.testInventory.getItem(40)));
                results.add(result);
                continue;
            }
            if (input.has("nbtMode")) {
                JsonObject spec = input.getAsJsonObject("nbtMode");
                String kind = spec.get("kind").getAsString();
                Tag tag = switch (kind) {
                    case "byte" -> ByteTag.valueOf(spec.get("value").getAsByte());
                    case "short" -> ShortTag.valueOf(spec.get("value").getAsShort());
                    case "int" -> IntTag.valueOf(spec.get("value").getAsInt());
                    case "long" -> LongTag.valueOf(spec.get("value").getAsLong());
                    case "float" -> FloatTag.valueOf(spec.get("value").getAsFloat());
                    case "double" -> DoubleTag.valueOf(spec.get("value").getAsDouble());
                    case "string" -> StringTag.valueOf(spec.get("value").getAsString());
                    default -> throw new IllegalArgumentException(kind);
                };
                var mode = GameType.LEGACY_ID_CODEC.parse(NbtOps.INSTANCE, tag).result();
                results.add(mode.isPresent() ? new JsonPrimitive(mode.get().getId()) : JsonNull.INSTANCE);
                continue;
            }
            if (input.has("mode")) {
                GameType mode = GameType.byId(input.get("mode").getAsInt());
                mode.updatePlayerAbilities(player.testAbilities);
                ItemStack item = stack(input.getAsJsonArray("item"));
                boolean placed = !mode.isBlockPlacingRestricted() && input.get("canPlace").getAsBoolean();
                if (placed) item.consume(1, player);
                JsonArray result = new JsonArray();
                result.add(mode.getId()); result.add(placed); result.add(output(item));
                results.add(result);
                continue;
            }
            if (input.has("result")) {
                ResultContainer inventory = new ResultContainer();
                inventory.setItem(0, stack(input.getAsJsonArray("result")));
                ResultMenu menu = new ResultMenu(inventory);
                menu.setCarried(stack(input.getAsJsonArray("cursor")));
                menu.clicked(0, input.get("button").getAsInt(), ContainerInput.PICKUP, player);
                JsonArray result = new JsonArray();
                result.add(output(menu.getSlot(0).getItem()));
                result.add(output(menu.getCarried()));
                results.add(result);
                continue;
            }
            if (input.has("slots")) {
                player.testInventory = new Inventory(player, new EntityEquipment());
                JsonArray source = input.getAsJsonArray("slots");
                for (int i=0; i<36; i++) player.testInventory.setItem(i, stack(source.get(i).getAsJsonArray()));
                InventoryMenu menu = new InventoryMenu(player.testInventory, false, player);
                int index = input.get("source").getAsInt();
                menu.clicked(index < 9 ? index + 36 : index, 0, ContainerInput.QUICK_MOVE, player);
                JsonArray result = new JsonArray();
                for (int i=0; i<36; i++) result.add(output(player.testInventory.getItem(i)));
                results.add(result);
                continue;
            }
            SimpleContainer inventory = new SimpleContainer(1);
            inventory.setItem(0, stack(input.getAsJsonArray("slot")));
            TestMenu menu = new TestMenu(inventory, input.get("limit").getAsInt(), input.get("place").getAsBoolean(), input.get("take").getAsBoolean());
            menu.setCarried(stack(input.getAsJsonArray("cursor")));
            menu.clicked(0, input.get("button").getAsInt(), ContainerInput.PICKUP, player);
            JsonArray result = new JsonArray(); result.add(output(menu.getSlot(0).getItem())); result.add(output(menu.getCarried())); results.add(result);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(results));
    }
}
"#;

fn java_bin() -> PathBuf {
    if let Some(path) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(path).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").expect("set MINECRAFT_JAVA_HOME to Java 25 JDK"))
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).unwrap()
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "inventory oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stack(spec: [u32; 2]) -> Option<ItemStack> {
    let (id, max) = match spec[0] {
        0 => return None,
        1 => ("minecraft:stone", 64),
        2 => ("minecraft:dirt", 64),
        3 => ("minecraft:ender_pearl", 16),
        4 => ("minecraft:stone", 16),
        5 => ("minecraft:stone_bricks", 64),
        6 => ("minecraft:stone", 99),
        7 => ("minecraft:stone", 64),
        8 => ("minecraft:diamond_sword", 1),
        9 => ("minecraft:apple", 64),
        _ => unreachable!(),
    };
    ItemStack::new(
        id.into(),
        spec[1],
        max,
        Tag::Compound(match spec[0] {
            4 | 6 | 7 => vec![("minecraft:max_stack_size".into(), Tag::Int(max as i32))],
            8 => vec![("minecraft:damage".into(), Tag::Int(13))],
            _ => vec![],
        }),
    )
}

fn output(stack: &Option<ItemStack>) -> Value {
    let Some(stack) = stack else {
        return json!([0, 0]);
    };
    let id = match (stack.id(), stack.max_count()) {
        ("minecraft:stone", Some(64)) => 1,
        ("minecraft:dirt", _) => 2,
        ("minecraft:ender_pearl", _) => 3,
        ("minecraft:stone", Some(16)) => 4,
        ("minecraft:stone_bricks", _) => 5,
        ("minecraft:stone", Some(99)) => 6,
        ("minecraft:diamond_sword", _) => 8,
        ("minecraft:apple", _) => 9,
        _ => unreachable!(),
    };
    json!([id, stack.count()])
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn pick_block_and_hotbar_selection_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    for seed in 0..270_u32 {
        let slots: Vec<Value> = (0..36_u32)
            .map(|index| {
                let code = (seed
                    .wrapping_mul(101)
                    .wrapping_add(index * 67)
                    .wrapping_add(seed * index * 7))
                    % 19;
                if code < seed % 8 {
                    json!([0, 0])
                } else {
                    json!([
                        match code % 5 {
                            0 => 1,
                            1 => 2,
                            2 => 4,
                            3 => 5,
                            _ => 9,
                        },
                        1 + code % 12
                    ])
                }
            })
            .collect();
        cases.push(json!({"pick": {"slots": slots, "selected": seed % 9, "creative": seed % 2 == 0, "item": [if seed % 3 == 0 { 1 } else { 5 }, 1]}}));
    }
    for selected in 0..9 {
        for creative in [false, true] {
            for scenario in 0..5 {
                let mut slots = vec![json!([2, 17]); 36];
                match scenario {
                    0 => slots[selected] = json!([0, 0]),
                    1 => slots[25] = json!([0, 0]),
                    2 => slots[15] = json!([1, 23]),
                    3 => {
                        slots[2] = json!([1, 5]);
                        slots[15] = json!([1, 23]);
                    }
                    _ => {}
                }
                cases.push(json!({"pick": {"slots": slots, "selected": selected, "creative": creative, "item": [1, 1]}}));
            }
        }
    }
    let mut block_ids: Vec<&str> = vec![
        "minecraft:grass_block",
        "minecraft:dirt",
        "minecraft:sand",
        "minecraft:red_sand",
        "minecraft:gravel",
        "minecraft:coarse_dirt",
        "minecraft:rooted_dirt",
        "minecraft:podzol",
        "minecraft:mycelium",
        "minecraft:dirt_path",
        "minecraft:farmland",
        "minecraft:stone",
        "minecraft:stone_bricks",
        "minecraft:cobblestone",
        "minecraft:crafting_table",
    ];
    block_ids.extend(
        native_core::wood::Planks::ALL
            .into_iter()
            .map(|block| block.id()),
    );
    block_ids.extend(native_core::wood::Wood::all().map(|block| block.id()));
    block_ids.extend(
        native_core::button::Button::ALL
            .into_iter()
            .map(|block| block.id()),
    );
    block_ids.extend([
        "minecraft:lever",
        "minecraft:redstone_block",
        "minecraft:redstone_lamp",
    ]);
    for id in &block_ids {
        cases.push(json!({"cloneBlock": id}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let output_file = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .args(["-encoding", "UTF-8", "-classpath"])
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&output_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(output_file).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (case, expected) in cases.iter().zip(expected) {
        if let Some(spec) = case.get("pick") {
            let mut inventory = PlayerInventory::default();
            for (index, value) in spec["slots"].as_array().unwrap().iter().enumerate() {
                let pair = [
                    value[0].as_u64().unwrap() as u32,
                    value[1].as_u64().unwrap() as u32,
                ];
                inventory.set_slot(PlayerSlot::Storage(index as u8), stack(pair));
            }
            inventory.select(spec["selected"].as_u64().unwrap() as u8);
            let item = &spec["item"];
            inventory.pick_item(
                stack([item[0].as_u64().unwrap() as u32, 1]).unwrap(),
                spec["creative"].as_bool().unwrap(),
            );
            let slots: Vec<Value> = (0..36)
                .map(|index| output(&inventory.slot(PlayerSlot::Storage(index)).cloned()))
                .collect();
            assert_eq!(
                json!([inventory.selected(), slots]),
                expected,
                "case {case}"
            );
        } else {
            let id = case["cloneBlock"].as_str().unwrap();
            let expected_id = id;
            for value in expected.as_array().unwrap() {
                assert_eq!(value.as_str().unwrap(), expected_id, "{id}");
            }
        }
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn explicit_default_stack_limit_matches_vanilla_component_equality() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let cases = vec![
        json!({"componentCompare":{}}),
        json!({"componentCompare":{"second":64}}),
        json!({"componentCompare":{"first":64,"second":64}}),
        json!({"componentCompare":{"first":99,"second":99}}),
        json!({"componentCompare":{"first":99,"second":64}}),
    ];
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let output_file = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&output_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(output_file).unwrap()).unwrap();
    for (case, expected) in cases.iter().zip(expected) {
        let spec = &case["componentCompare"];
        let make = |key: &str| {
            let value = spec.get(key).and_then(Value::as_u64).map(|n| n as u32);
            let components = Tag::Compound(
                value
                    .into_iter()
                    .map(|n| ("minecraft:max_stack_size".into(), Tag::Int(n as i32)))
                    .collect(),
            );
            ItemStack::new("minecraft:stone".into(), 1, value.unwrap_or(64), components).unwrap()
        };
        assert_eq!(
            make("first").same_item_and_components(&make("second")),
            expected.as_bool().unwrap(),
            "case {case}"
        );
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn hotbar_and_offhand_swap_match_vanilla_containers() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let choices = [
        [0, 0],
        [1, 1],
        [1, 32],
        [1, 64],
        [2, 7],
        [3, 16],
        [4, 16],
        [1, 70],
        [4, 19],
        [6, 1],
        [6, 70],
        [6, 99],
    ];
    let mut cases = Vec::new();
    for source in [0, 8, 9, 17, 35, 36, 37, 38, 39, 40] {
        for target in [0, 8, 40] {
            for source_item in choices {
                for target_item in choices {
                    for full in [false, true] {
                        let mut slots = vec![if full { [1, 64] } else { [0, 0] }; 36];
                        let mut offhand = if full { [1, 64] } else { [0, 0] };
                        let mut craft = vec![[0, 0]; 4];
                        if source >= 37 {
                            craft[source as usize - 37] = source_item;
                        } else if source == 36 {
                            offhand = source_item;
                        } else {
                            slots[source as usize] = source_item;
                        }
                        if target == 40 {
                            offhand = target_item;
                        } else {
                            slots[target as usize] = target_item;
                        }
                        let creative = (cases.len() % 3) == 0;
                        let cursor = if cases.len() % 7 == 0 { [2, 1] } else { [0, 0] };
                        cases.push(json!({"swapCase":{"screen":true,"slots":slots,"offhand":offhand,"craft":craft,"selected":3,"source":source,"target":target,"cursor":cursor,"creative":creative}}));
                    }
                }
            }
        }
    }
    for selected in 0..9 {
        for main in choices {
            for offhand in choices {
                let mut slots = vec![[0, 0]; 36];
                slots[selected] = main;
                let craft = vec![[0, 0]; 4];
                cases.push(json!({"swapCase":{"screen":false,"slots":slots,"offhand":offhand,"craft":craft,"selected":selected,"source":0,"target":40,"cursor":[0,0],"creative":false}}));
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let expected_file = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&expected_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(expected_file).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    let parse = |value: &Value| {
        stack([
            value[0].as_u64().unwrap() as u32,
            value[1].as_u64().unwrap() as u32,
        ])
    };
    let mut differences = Vec::new();
    for (case, expected) in cases.iter().zip(&expected) {
        let case = &case["swapCase"];
        let mut inventory = PlayerInventory::default();
        for (index, item) in case["slots"].as_array().unwrap().iter().enumerate() {
            inventory.set_slot(PlayerSlot::Storage(index as u8), parse(item));
        }
        inventory.set_slot(PlayerSlot::Offhand, parse(&case["offhand"]));
        inventory.select(case["selected"].as_u64().unwrap() as u8);
        let dropped = if case["screen"].as_bool().unwrap() {
            inventory.toggle();
            for (index, item) in case["craft"].as_array().unwrap().iter().enumerate() {
                inventory.set_slot(PlayerSlot::Crafting(index as u8), parse(item));
            }
            if case["cursor"][0] != 0 {
                let previous = inventory.slot(PlayerSlot::Storage(35)).cloned();
                inventory.set_slot(PlayerSlot::Storage(35), parse(&case["cursor"]));
                assert!(inventory.click(PlayerSlot::Storage(35), ClickButton::Primary));
                inventory.set_slot(PlayerSlot::Storage(35), previous);
            }
            let source = case["source"].as_u64().unwrap() as u8;
            let target = case["target"].as_u64().unwrap() as u8;
            inventory
                .swap_slot(
                    if source >= 37 {
                        PlayerSlot::Crafting(source - 37)
                    } else if source == 36 {
                        PlayerSlot::Offhand
                    } else {
                        PlayerSlot::Storage(source)
                    },
                    if target == 40 {
                        PlayerSlot::Offhand
                    } else {
                        PlayerSlot::Storage(target)
                    },
                    case["creative"].as_bool().unwrap(),
                    None,
                )
                .unwrap_or_default()
        } else {
            inventory.swap_hands();
            Vec::new()
        };
        assert!(dropped.len() <= 1);
        let slots: Vec<_> = (0..36)
            .map(|i| output(&inventory.slot(PlayerSlot::Storage(i)).cloned()))
            .collect();
        let craft: Vec<_> = (0..4)
            .map(|i| output(&inventory.slot(PlayerSlot::Crafting(i)).cloned()))
            .collect();
        let actual = json!([
            slots,
            output(&inventory.slot(PlayerSlot::Offhand).cloned()),
            output(&inventory.cursor().cloned()),
            output(&dropped.first().cloned()),
            craft
        ]);
        if &actual != expected {
            differences.push(format!("case {case}: actual {actual}, expected {expected}"));
        }
    }
    assert!(
        differences.is_empty(),
        "{} differences:\n{}",
        differences.len(),
        differences
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!(
        "Compared {} hand and inventory swaps with vanilla 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn craft_result_shortcut_matches_vanilla_result_slot() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let recipes = native_resources::recipes::CraftingRecipes::load(&mut resources).unwrap();
    let mut cases = Vec::new();
    for target in [0, 8, 40] {
        for destination in [[0, 0], [1, 1], [5, 1], [5, 60], [5, 64]] {
            for cursor in [[0, 0], [1, 1], [2, 64]] {
                cases.push(json!({"swapResult":{"target":target,"destination":destination,"output":[5,4],"cursor":cursor}}));
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let output_file = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&output_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(output_file).unwrap()).unwrap();
    for (case, expected) in cases.iter().zip(&expected) {
        let case = &case["swapResult"];
        let parse =
            |v: &Value| stack([v[0].as_u64().unwrap() as u32, v[1].as_u64().unwrap() as u32]);
        let target = case["target"].as_u64().unwrap() as u8;
        let target = if target == 40 {
            PlayerSlot::Offhand
        } else {
            PlayerSlot::Storage(target)
        };
        let mut inventory = PlayerInventory::default();
        inventory.toggle();
        for index in 0..4 {
            inventory.set_slot(
                PlayerSlot::Crafting(index),
                ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])),
            );
        }
        inventory.set_slot(target, parse(&case["destination"]));
        if case["cursor"][0] != 0 {
            inventory.set_slot(PlayerSlot::Storage(35), parse(&case["cursor"]));
            assert!(inventory.click(PlayerSlot::Storage(35), ClickButton::Primary));
        }
        inventory.refresh_crafting(&recipes);
        let changed = inventory
            .swap_slot(PlayerSlot::CraftResult, target, false, Some(&recipes))
            .is_some();
        let actual = json!([
            output(&inventory.slot(target).cloned()),
            output(&inventory.slot(PlayerSlot::CraftResult).cloned()),
            output(&inventory.cursor().cloned())
        ]);
        assert_eq!(&actual, expected, "case {case}");
        for index in 0..4 {
            assert_eq!(
                inventory.slot(PlayerSlot::Crafting(index)).is_none(),
                changed,
                "case {case}"
            );
        }
    }
    println!(
        "Compared {} result-slot shortcuts with vanilla 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn chest_shift_clicks_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let choices = [
        [0, 0],
        [1, 1],
        [1, 32],
        [1, 64],
        [2, 8],
        [2, 64],
        [5, 63],
        [6, 90],
    ];
    let mut cases = Vec::new();
    for chest_size in [27, 54] {
        for seed in 0..40_usize {
            for source in 0..36 + chest_size {
                let mut slots: Vec<_> = (0..36)
                    .map(|i| choices[(seed * 7 + i * 3) % choices.len()])
                    .collect();
                let mut chest: Vec<_> = (0..chest_size)
                    .map(|i| choices[(seed * 5 + i * 7) % choices.len()])
                    .collect();
                let item = choices[1 + seed % (choices.len() - 1)];
                if source < 36 {
                    slots[source] = item;
                } else {
                    chest[source - 36] = item;
                }
                cases.push(json!({"chestMove":{"slots":slots,"chest":chest,"source":source}}));
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source_file = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let output_file = temp.path().join("output.json");
    std::fs::write(&source_file, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source_file));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&output_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(output_file).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    let parse = |value: &Value| {
        stack([
            value[0].as_u64().unwrap() as u32,
            value[1].as_u64().unwrap() as u32,
        ])
    };
    for (case, expected) in cases.iter().zip(&expected) {
        let spec = &case["chestMove"];
        let mut inventory = PlayerInventory::default();
        let count = spec["chest"].as_array().unwrap().len();
        let chest = std::array::from_fn(|i| parse(&spec["chest"][i]));
        if count == 27 {
            assert!(inventory.open_chest(glam::IVec3::ZERO, chest));
        } else {
            assert!(inventory.open_double_chest(
                glam::IVec3::ZERO,
                glam::IVec3::X,
                chest,
                std::array::from_fn(|i| parse(&spec["chest"][i + 27]))
            ));
        }
        for (i, item) in spec["slots"].as_array().unwrap().iter().enumerate() {
            inventory.set_slot(PlayerSlot::Storage(i as u8), parse(item));
        }
        let source = spec["source"].as_u64().unwrap() as u8;
        inventory.quick_move(if source < 36 {
            PlayerSlot::Storage(source)
        } else {
            PlayerSlot::Chest(source - 36)
        });
        let slots: Vec<_> = (0..36)
            .map(|i| output(&inventory.slot(PlayerSlot::Storage(i)).cloned()))
            .collect();
        let chest: Vec<_> = (0..count as u8)
            .map(|i| output(&inventory.slot(PlayerSlot::Chest(i)).cloned()))
            .collect();
        assert_eq!(&json!([slots, chest]), expected, "case {case}");
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn crafting_menu_shift_clicks_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let recipes = native_resources::recipes::CraftingRecipes::load(&mut resources).unwrap();
    let choices = [
        [0, 0],
        [1, 1],
        [1, 32],
        [1, 64],
        [2, 8],
        [2, 64],
        [5, 63],
        [6, 90],
    ];
    let mut cases = Vec::new();
    for seed in 0..48_usize {
        for source in (0..36).step_by(7).chain(36..45) {
            let mut slots: Vec<_> = (0..36)
                .map(|i| choices[(seed * 7 + i * 3) % choices.len()])
                .collect();
            let mut craft: Vec<_> = (0..9)
                .map(|i| choices[(seed * 5 + i * 7) % choices.len()])
                .collect();
            let item = choices[1 + seed % (choices.len() - 1)];
            if source < 36 {
                slots[source] = item;
            } else {
                craft[source - 36] = item;
            }
            cases.push(json!({"tableMove":{"slots":slots,"craft":craft,"source":source}}));
        }
    }
    for target in [0, 8, 9, 20, 35] {
        for choice in [[0, 0], [5, 60], [5, 62], [5, 63], [5, 64]] {
            let mut slots = vec![[2, 64]; 36];
            slots[target] = choice;
            cases.push(json!({"tableResult":{"slots":slots}}));
            cases.push(json!({"playerResult":{"slots":slots}}));
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source_file = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let output_file = temp.path().join("output.json");
    std::fs::write(&source_file, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source_file));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&output_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(output_file).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    let parse = |value: &Value| {
        stack([
            value[0].as_u64().unwrap() as u32,
            value[1].as_u64().unwrap() as u32,
        ])
    };
    for (case, expected) in cases.iter().zip(&expected) {
        let spec = case
            .get("tableMove")
            .or_else(|| case.get("tableResult"))
            .or_else(|| case.get("playerResult"))
            .unwrap();
        let mut inventory = PlayerInventory::default();
        let player_menu = case.get("playerResult").is_some();
        if player_menu {
            assert!(inventory.toggle());
        } else {
            assert!(inventory.open_crafting_table(glam::IVec3::ZERO));
        }
        for (index, item) in spec["slots"].as_array().unwrap().iter().enumerate() {
            inventory.set_slot(PlayerSlot::Storage(index as u8), parse(item));
        }
        let drops = if let Some(craft) = spec.get("craft") {
            for (index, item) in craft.as_array().unwrap().iter().enumerate() {
                inventory.set_slot(PlayerSlot::Crafting(index as u8), parse(item));
            }
            let source = spec["source"].as_u64().unwrap() as u8;
            inventory.quick_move(if source < 36 {
                PlayerSlot::Storage(source)
            } else {
                PlayerSlot::Crafting(source - 36)
            });
            Vec::new()
        } else {
            let ingredients = if player_menu {
                [0, 1, 2, 3]
            } else {
                [0, 1, 3, 4]
            };
            for index in ingredients {
                inventory.set_slot(
                    PlayerSlot::Crafting(index),
                    ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])),
                );
            }
            inventory
                .quick_crafting_result(&recipes)
                .unwrap_or_default()
        };
        let slots: Vec<_> = (0..36)
            .map(|i| output(&inventory.slot(PlayerSlot::Storage(i)).cloned()))
            .collect();
        let craft: Vec<_> = (0..if player_menu { 4 } else { 9 })
            .map(|i| output(&inventory.slot(PlayerSlot::Crafting(i)).cloned()))
            .collect();
        let actual = if case.get("tableMove").is_some() {
            json!([slots, craft])
        } else {
            json!([slots, craft, output(&drops.first().cloned())])
        };
        assert_eq!(&actual, expected, "case {case}");
    }
    println!(
        "Compared {} crafting menu shift-click cases with Java 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn recipe_book_placement_matches_vanilla() {
    use native_resources::packs::ResourceId;
    use native_session::inventory::{RecipeBookPlacement, RecipeBookState};

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let recipes = native_resources::recipes::CraftingRecipes::load(&mut resources).unwrap();
    let id = ResourceId::parse("minecraft:stone_bricks").unwrap();
    let recipe = recipes.get(&id).unwrap();
    let mut cases = Vec::new();
    for size in [2_usize, 3] {
        for use_max in [false, true] {
            for count in [0, 1, 2, 4, 8, 16, 63, 64] {
                for grid_kind in 0..4 {
                    let mut slots = vec![[0, 0]; 36];
                    if count > 0 {
                        slots[0] = [1, count];
                    }
                    slots[1] = [2, 5];
                    let mut craft = vec![[0, 0]; size * size];
                    match grid_kind {
                        1 | 2 => {
                            for index in [0, 1, size, size + 1] {
                                craft[index] = [1, if grid_kind == 1 { 1 } else { 3 }];
                            }
                        }
                        3 => craft[0] = [2, 2],
                        _ => {}
                    }
                    cases.push(json!({"bookPlace":{"size":size,"max":use_max,"slots":slots,"craft":craft}}));
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source_file = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let output_file = temp.path().join("output.json");
    std::fs::write(&source_file, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source_file));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&output_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(output_file).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    let parse = |value: &Value| {
        stack([
            value[0].as_u64().unwrap() as u32,
            value[1].as_u64().unwrap() as u32,
        ])
    };
    for (case, expected) in cases.iter().zip(expected) {
        let spec = &case["bookPlace"];
        let size = spec["size"].as_u64().unwrap() as usize;
        let mut inventory = PlayerInventory::default();
        let mut book = RecipeBookState::default();
        book.known.insert(id.clone());
        inventory.set_recipe_book(book);
        if size == 2 {
            assert!(inventory.toggle());
        } else {
            assert!(inventory.open_crafting_table(glam::IVec3::ZERO));
        }
        for (index, value) in spec["slots"].as_array().unwrap().iter().enumerate() {
            inventory.set_slot(PlayerSlot::Storage(index as u8), parse(value));
        }
        for (index, value) in spec["craft"].as_array().unwrap().iter().enumerate() {
            inventory.set_slot(PlayerSlot::Crafting(index as u8), parse(value));
        }
        let action = inventory.place_book_recipe(recipe, &recipes, spec["max"].as_bool().unwrap());
        let slots: Vec<_> = (0..36)
            .map(|i| output(&inventory.slot(PlayerSlot::Storage(i)).cloned()))
            .collect();
        let craft: Vec<_> = (0..size * size)
            .map(|i| output(&inventory.slot(PlayerSlot::Crafting(i as u8)).cloned()))
            .collect();
        assert_eq!(
            json!([slots, craft, action == RecipeBookPlacement::Ghost]),
            expected,
            "{case}"
        );
    }
    println!(
        "Compared {} recipe-book placements with Java 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn normal_slot_pickup_matches_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let choices = [
        [0, 0],
        [1, 1],
        [1, 3],
        [1, 32],
        [1, 64],
        [2, 1],
        [2, 3],
        [2, 64],
        [3, 1],
        [3, 8],
        [3, 16],
        [4, 1],
        [4, 7],
        [4, 16],
        [6, 1],
        [6, 70],
        [6, 99],
        [7, 1],
        [7, 63],
        [8, 1],
        [9, 1],
        [9, 63],
        [9, 64],
    ];
    let mut cases = Vec::new();
    for slot in choices {
        for cursor in choices {
            for limit in [1, 16, 64, 99] {
                for place in [false, true] {
                    for take in [false, true] {
                        for button in 0..=1 {
                            cases.push(json!({"slot":slot,"cursor":cursor,"limit":limit,"place":place,"take":take,"button":button}));
                        }
                    }
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let expected_file = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&expected_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(expected_file).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    let mut differences = Vec::new();
    for (case, expected) in cases.iter().zip(expected.iter()) {
        let array = |name: &str| -> [u32; 2] {
            [
                case[name][0].as_u64().unwrap() as u32,
                case[name][1].as_u64().unwrap() as u32,
            ]
        };
        let mut slot = stack(array("slot"));
        let mut cursor = stack(array("cursor"));
        let place = case["place"].as_bool().unwrap();
        let rules = SlotRules {
            max_count: case["limit"].as_u64().unwrap() as u32,
            may_take: case["take"].as_bool().unwrap(),
            accepts_carried: place,
            accepts_existing: place,
        };
        let button = if case["button"] == 0 {
            ClickButton::Primary
        } else {
            ClickButton::Secondary
        };
        pickup(&mut slot, &mut cursor, button, rules);
        let actual = json!([output(&slot), output(&cursor)]);
        if &actual != expected {
            differences.push(format!("case {case}: actual {actual}, expected {expected}"));
        }
    }
    assert!(
        differences.is_empty(),
        "{} differences:\n{}",
        differences.len(),
        differences
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!("Compared {} ordinary clicks with vanilla 26.3", cases.len());
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn shift_click_between_inventory_and_hotbar_matches_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let choices = [
        [0, 0],
        [0, 0],
        [0, 0],
        [1, 1],
        [1, 3],
        [1, 32],
        [1, 63],
        [1, 64],
        [2, 1],
        [2, 32],
        [2, 64],
        [4, 1],
        [4, 7],
        [4, 16],
        [6, 1],
        [6, 70],
        [6, 99],
        [7, 1],
        [7, 63],
        [8, 1],
        [9, 1],
        [9, 63],
        [9, 64],
    ];
    let mut cases = Vec::new();
    for seed in 0..512_u32 {
        let mut random = seed.wrapping_add(1);
        let mut values = Vec::new();
        for _ in 0..36 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            values.push(choices[(random >> 16) as usize % choices.len()]);
        }
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let source = (random >> 16) as usize % 36;
        values[source] = choices[3 + seed as usize % (choices.len() - 3)];
        cases.push(json!({"slots":values,"source":source}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source_file = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let expected_file = temp.path().join("output.json");
    std::fs::write(&source_file, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source_file));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&expected_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(expected_file).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    let mut differences = Vec::new();
    for (case, expected) in cases.iter().zip(expected.iter()) {
        let mut inventory = PlayerInventory::default();
        for (index, spec) in case["slots"].as_array().unwrap().iter().enumerate() {
            let spec = [
                spec[0].as_u64().unwrap() as u32,
                spec[1].as_u64().unwrap() as u32,
            ];
            inventory.set_slot(PlayerSlot::Storage(index as u8), stack(spec));
        }
        inventory.toggle();
        inventory.quick_move(PlayerSlot::Storage(case["source"].as_u64().unwrap() as u8));
        let actual: Vec<_> = (0..36)
            .map(|index| output(&inventory.slot(PlayerSlot::Storage(index)).cloned()))
            .collect();
        if json!(actual) != *expected {
            differences.push(format!(
                "case {case}: actual {actual:?}, expected {expected}"
            ));
        }
    }
    assert!(
        differences.is_empty(),
        "{} differences:\n{}",
        differences.len(),
        differences
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!(
        "Compared {} shift-click inventories with vanilla 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn craft_result_pickup_matches_vanilla_result_container() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let recipes = native_resources::recipes::CraftingRecipes::load(&mut resources).unwrap();
    let mut cursors = vec![[0, 0]];
    for id in [1, 2, 5] {
        for count in [1, 59, 60, 61, 63, 64] {
            cursors.push([id, count]);
        }
    }
    let cases: Vec<_> = (0..=1)
        .flat_map(|button| {
            cursors
                .iter()
                .map(move |cursor| json!({"result":[5,4], "cursor":cursor, "button":button}))
        })
        .collect();
    let temp = tempfile::tempdir().unwrap();
    let source_file = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let expected_file = temp.path().join("output.json");
    std::fs::write(&source_file, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source_file));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&expected_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(expected_file).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    for (case, expected) in cases.iter().zip(expected.iter()) {
        let mut inventory = PlayerInventory::default();
        inventory.toggle();
        for index in 0..4 {
            inventory.set_slot(
                PlayerSlot::Crafting(index),
                ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])),
            );
        }
        let cursor = &case["cursor"];
        let id = cursor[0].as_u64().unwrap();
        if id != 0 {
            let name = match id {
                1 => "minecraft:stone",
                2 => "minecraft:dirt",
                5 => "minecraft:stone_bricks",
                _ => unreachable!(),
            };
            inventory.set_slot(
                PlayerSlot::Storage(10),
                ItemStack::new(
                    name.into(),
                    cursor[1].as_u64().unwrap() as u32,
                    64,
                    Tag::Compound(vec![]),
                ),
            );
            inventory.click(PlayerSlot::Storage(10), ClickButton::Primary);
        }
        inventory.refresh_crafting(&recipes);
        let took = inventory.take_crafting_result(&recipes).is_some();
        let actual = json!([
            output(&inventory.slot(PlayerSlot::CraftResult).cloned()),
            output(&inventory.cursor().cloned())
        ]);
        assert_eq!(&actual, expected, "case {case}");
        for index in 0..4 {
            assert_eq!(inventory.slot(PlayerSlot::Crafting(index)).is_none(), took);
        }
    }
    println!(
        "Compared {} result-slot clicks with vanilla 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn block_placement_item_consumption_matches_vanilla_abilities() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    for mode in [-1, 0, 1, 2, 3, 4, i32::MAX] {
        for count in [1, 2, 64] {
            for can_place in [false, true] {
                cases.push(json!({"mode":mode,"item":[5,count],"canPlace":can_place}));
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let expected_file = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&expected_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(expected_file).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    for (case, expected) in cases.iter().zip(expected.iter()) {
        let mut world = World::default();
        let can_place = case["canPlace"].as_bool().unwrap();
        let wall = glam::IVec3::new(0, 4, if can_place { 3 } else { 4 });
        let target = wall + glam::IVec3::Z;
        world.set_block(wall, Block::Stone);
        let mut session = LocalSession::new(world);
        let mode = GameMode::from_legacy_id(case["mode"].as_i64().unwrap() as i32);
        let mode_id = match mode {
            GameMode::Survival => 0,
            GameMode::Creative => 1,
            GameMode::Adventure => 2,
            GameMode::Spectator => 3,
        };
        session.set_game_mode(mode);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new(
                "minecraft:stone_bricks".into(),
                case["item"][1].as_u64().unwrap() as u32,
                64,
                Tag::Compound(vec![]),
            ),
        );
        session.set_player_inventory(inventory);
        let before = session.snapshot();
        session.submit_command(PlayerCommand::Place);
        session.update(std::time::Duration::from_millis(50));
        let after = session.snapshot();
        let placed = after.world.demo().unwrap().block(target) == Block::StoneBricks;
        let actual = json!([
            mode_id,
            placed,
            output(&after.inventory.slot(PlayerSlot::Storage(0)).cloned())
        ]);
        assert_eq!(&actual, expected, "case {case}");
        assert_eq!(
            before
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            case["item"][1].as_u64().unwrap() as u32
        );
    }
    println!(
        "Compared {} placement and item-consumption cases with vanilla abilities",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn saved_game_mode_numbers_match_vanilla_nbt_codec() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let specs = [
        ("byte", json!(-1), Tag::Byte(-1)),
        ("byte", json!(2), Tag::Byte(2)),
        ("short", json!(3), Tag::Short(3)),
        ("int", json!(0), Tag::Int(0)),
        ("int", json!(1), Tag::Int(1)),
        ("int", json!(99), Tag::Int(99)),
        ("long", json!(4294967297_u64), Tag::Long(4294967297)),
        ("float", json!(-0.9), Tag::Float(-0.9)),
        ("float", json!(1.75), Tag::Float(1.75)),
        ("double", json!(3.9), Tag::Double(3.9)),
        ("string", json!("creative"), Tag::String("creative".into())),
    ];
    let cases: Vec<_> = specs
        .iter()
        .map(|(kind, value, _)| json!({"nbtMode":{"kind":kind,"value":value}}))
        .collect();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let expected_file = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&expected_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(expected_file).unwrap()).unwrap();
    assert_eq!(specs.len(), expected.len());
    for ((kind, value, tag), expected) in specs.iter().zip(expected) {
        let actual = tag
            .as_java_int()
            .map(GameMode::from_legacy_id)
            .map(|mode| match mode {
                GameMode::Survival => 0,
                GameMode::Creative => 1,
                GameMode::Adventure => 2,
                GameMode::Spectator => 3,
            });
        assert_eq!(json!(actual), expected, "{kind} {value}");
    }
    println!(
        "Compared {} saved mode values with the vanilla NBT codec",
        specs.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn dropped_stack_pickup_matches_vanilla_inventory_add() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let choices = [
        [0, 0],
        [0, 0],
        [1, 1],
        [1, 32],
        [1, 63],
        [1, 64],
        [2, 1],
        [2, 63],
        [2, 64],
        [5, 1],
        [5, 63],
        [5, 64],
        [6, 1],
        [6, 70],
        [6, 99],
    ];
    let drops = [
        [0, 0],
        [1, 1],
        [1, 3],
        [1, 64],
        [1, 99],
        [2, 1],
        [2, 33],
        [5, 1],
        [5, 65],
        [6, 1],
        [6, 70],
        [6, 99],
    ];
    let mut cases = Vec::new();
    for seed in 0..640_u32 {
        let mut random = seed + 1;
        let mut slots = Vec::new();
        for index in 0..36 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            slots.push(if seed % 5 == 0 {
                if index % 2 == 0 { [1, 64] } else { [2, 64] }
            } else {
                choices[(random >> 16) as usize % choices.len()]
            });
        }
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let selected = (random >> 16) % 9;
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let offhand = choices[(random >> 16) as usize % choices.len()];
        cases.push(json!({"slots":slots, "selected":selected, "offhand":offhand, "pickup":drops[seed as usize % drops.len()], "creative":seed % 2 == 0}));
    }
    // In particular, a partly filled inventory makes Java return false in
    // survival even though it moved items, but discard the remainder in creative.
    for creative in [false, true] {
        for first_count in [63, 64] {
            let mut slots = vec![[2, 64]; 36];
            slots[0] = [1, first_count];
            cases.push(json!({"slots":slots, "selected":0, "offhand":[2,64], "pickup":[1,3], "creative":creative}));
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let expected_file = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&expected_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(expected_file).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    let parse_stack = |value: &Value| {
        stack([
            value[0].as_u64().unwrap() as u32,
            value[1].as_u64().unwrap() as u32,
        ])
    };
    for (case, expected) in cases.iter().zip(expected) {
        let mut inventory = PlayerInventory::default();
        for (index, value) in case["slots"].as_array().unwrap().iter().enumerate() {
            inventory.set_slot(PlayerSlot::Storage(index as u8), parse_stack(value));
        }
        inventory.set_slot(PlayerSlot::Offhand, parse_stack(&case["offhand"]));
        inventory.select(case["selected"].as_u64().unwrap() as u8);
        let mut drop = parse_stack(&case["pickup"]);
        let changed = inventory.pickup_stack(&mut drop, case["creative"].as_bool().unwrap());
        let slots: Vec<_> = (0..36)
            .map(|index| output(&inventory.slot(PlayerSlot::Storage(index)).cloned()))
            .collect();
        let actual = json!([
            changed,
            output(&drop),
            slots,
            output(&inventory.slot(PlayerSlot::Offhand).cloned())
        ]);
        assert_eq!(actual, expected, "case {case}");
    }
    println!(
        "Compared {} dropped-stack pickups with vanilla 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn dropping_and_returning_items_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let choices = [[0, 0], [1, 1], [1, 32], [1, 64], [2, 1], [2, 63], [5, 4]];
    let mut cases = Vec::new();
    for kind in ["selected", "slot", "outside"] {
        for slot in choices {
            for cursor in choices {
                for all in [false, true] {
                    if kind == "selected" && cursor != [0, 0] {
                        continue;
                    }
                    cases.push(json!({"dropKind":kind,"slot":slot,"cursor":cursor,"all":all}));
                }
            }
        }
    }
    for seed in 0..320_u32 {
        let mut random = seed + 1;
        let mut slots = Vec::new();
        for index in 0..36 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            slots.push(if seed % 5 == 0 {
                if index % 2 == 0 { [1, 64] } else { [2, 64] }
            } else {
                choices[(random >> 16) as usize % choices.len()]
            });
        }
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let selected = (random >> 16) % 9;
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let offhand = choices[(random >> 16) as usize % choices.len()];
        cases.push(json!({"returnStack":choices[1 + seed as usize % (choices.len()-1)], "slots":slots, "selected":selected, "offhand":offhand}));
    }
    // Only one item can fit, so the remainder must be dropped intact.
    let mut nearly_full = vec![[2, 64]; 36];
    nearly_full[3] = [1, 63];
    cases.push(json!({"returnStack":[1,64], "slots":nearly_full, "selected":3, "offhand":[0,0]}));
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let expected_file = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&expected_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(expected_file).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    let parse = |value: &Value| {
        stack([
            value[0].as_u64().unwrap() as u32,
            value[1].as_u64().unwrap() as u32,
        ])
    };
    for (case, expected) in cases.iter().zip(expected) {
        let mut inventory = PlayerInventory::default();
        let actual = if let Some(kind) = case.get("dropKind").and_then(Value::as_str) {
            inventory.set_slot(PlayerSlot::Storage(0), parse(&case["slot"]));
            let all = case["all"].as_bool().unwrap();
            if kind == "selected" {
                let dropped = inventory.drop_selected(all);
                json!([
                    output(&inventory.slot(PlayerSlot::Storage(0)).cloned()),
                    output(&dropped)
                ])
            } else {
                inventory.set_slot(PlayerSlot::Storage(1), parse(&case["cursor"]));
                inventory.toggle();
                inventory.click(PlayerSlot::Storage(1), ClickButton::Primary);
                let dropped = if kind == "slot" {
                    inventory.throw_slot(PlayerSlot::Storage(0), all)
                } else {
                    inventory.drop_cursor(if all {
                        ClickButton::Secondary
                    } else {
                        ClickButton::Primary
                    })
                };
                json!([
                    output(&inventory.slot(PlayerSlot::Storage(0)).cloned()),
                    output(&inventory.cursor().cloned()),
                    output(&dropped)
                ])
            }
        } else {
            for (index, value) in case["slots"].as_array().unwrap().iter().enumerate() {
                inventory.set_slot(PlayerSlot::Storage(index as u8), parse(value));
            }
            inventory.set_slot(PlayerSlot::Offhand, parse(&case["offhand"]));
            inventory.select(case["selected"].as_u64().unwrap() as u8);
            inventory.set_slot(PlayerSlot::Crafting(0), parse(&case["returnStack"]));
            inventory.toggle();
            let mut drops = inventory.toggle_with_drops().unwrap();
            assert!(drops.len() <= 1);
            let slots: Vec<_> = (0..36)
                .map(|slot| output(&inventory.slot(PlayerSlot::Storage(slot)).cloned()))
                .collect();
            json!([
                slots,
                output(&inventory.slot(PlayerSlot::Offhand).cloned()),
                output(&drops.pop())
            ])
        };
        assert_eq!(actual, expected, "case {case}");
    }
    println!(
        "Compared {} selected, slot, outside-click and return cases with vanilla",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn inventory_drag_matches_vanilla_quick_craft() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let cursors = [
        [1, 1],
        [1, 2],
        [1, 3],
        [1, 8],
        [1, 32],
        [1, 64],
        [3, 16],
        [4, 16],
        [6, 99],
        [8, 1],
    ];
    let mut cases = Vec::new();
    for table in [false, true] {
        for button in [0, 1] {
            for cursor in cursors {
                for seed in 0..24_u32 {
                    let max = match cursor[0] {
                        3 | 4 => 16,
                        6 => 99,
                        8 => 1,
                        _ => 64,
                    };
                    let variants = [
                        [0, 0],
                        [cursor[0], 1],
                        [cursor[0], max - 1],
                        [cursor[0], max],
                        [2, 5],
                        [1, 5],
                        [4, 4],
                    ];
                    let mut slots = vec![[0, 0]; 36];
                    let mut craft = vec![[0, 0]; if table { 9 } else { 4 }];
                    for (i, value) in slots.iter_mut().enumerate().take(6) {
                        *value = variants[(i + seed as usize) % variants.len()];
                    }
                    for (i, value) in craft.iter_mut().enumerate() {
                        *value = variants[(i * 3 + seed as usize) % variants.len()];
                    }
                    let offhand = variants[(seed as usize + 2) % variants.len()];
                    let mut visits = match seed % 6 {
                        0 => vec![0],
                        1 => vec![0, 1, 2],
                        2 => vec![0, 0, 1, 1, 2, 2],
                        3 => vec![36, 37, 0, 1],
                        4 => vec![101, 0, 36, 2, 38],
                        _ => vec![3, 4, 5, 36, 37, 38, 39],
                    };
                    if !table && seed % 3 == 0 {
                        visits.push(100);
                    }
                    cases.push(json!({"drag": {"table":table,"button":button,"slots":slots,
                        "craft":craft,"offhand":offhand,"cursor":cursor,"visits":visits}}));
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("InventoryOracle.java");
    let input = temp.path().join("input.json");
    let expected_file = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let java = java_bin();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InventoryOracle")
        .arg(&input)
        .arg(&expected_file));
    let expected: Vec<Value> =
        serde_json::from_slice(&std::fs::read(expected_file).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    let parse = |value: &Value| {
        stack([
            value[0].as_u64().unwrap() as u32,
            value[1].as_u64().unwrap() as u32,
        ])
    };
    for (case, expected) in cases.iter().zip(expected) {
        let spec = &case["drag"];
        let mut inventory = PlayerInventory::default();
        if spec["table"].as_bool().unwrap() {
            assert!(inventory.open_crafting_table(glam::IVec3::new(0, 4, 3)));
        } else {
            inventory.toggle();
        }
        for (index, value) in spec["slots"].as_array().unwrap().iter().enumerate() {
            inventory.set_slot(PlayerSlot::Storage(index as u8), parse(value));
        }
        for (index, value) in spec["craft"].as_array().unwrap().iter().enumerate() {
            inventory.set_slot(PlayerSlot::Crafting(index as u8), parse(value));
        }
        inventory.set_slot(PlayerSlot::Offhand, parse(&spec["offhand"]));
        let last = inventory.slot(PlayerSlot::Storage(35)).cloned();
        inventory.set_slot(PlayerSlot::Storage(35), parse(&spec["cursor"]));
        inventory.click(PlayerSlot::Storage(35), ClickButton::Primary);
        inventory.set_slot(PlayerSlot::Storage(35), last);
        let button = if spec["button"] == 0 {
            ClickButton::Primary
        } else {
            ClickButton::Secondary
        };
        let mut world = World::demo();
        if spec["table"].as_bool().unwrap() {
            world.set_block(glam::IVec3::new(0, 4, 3), Block::CraftingTable);
        }
        let mut session = LocalSession::new(world);
        session.set_player_inventory(inventory);
        session.submit_command(PlayerCommand::InventoryPointer(
            InventoryPointerEvent::Press {
                target: InventoryPointerTarget::Slot(PlayerSlot::Storage(0)),
                button,
                shift: false,
            },
        ));
        for target in spec["visits"].as_array().unwrap() {
            let target = target.as_u64().unwrap() as u8;
            let slot = match target {
                100 => PlayerSlot::Offhand,
                101 => PlayerSlot::CraftResult,
                36..=44 => PlayerSlot::Crafting(target - 36),
                _ => PlayerSlot::Storage(target),
            };
            session.submit_command(PlayerCommand::InventoryPointer(
                InventoryPointerEvent::Move { slot },
            ));
        }
        session.submit_command(PlayerCommand::InventoryPointer(
            InventoryPointerEvent::Release {
                target: InventoryPointerTarget::Background,
                button,
                shift: false,
            },
        ));
        session.update(std::time::Duration::from_millis(50));
        let snapshot = session.snapshot();
        let inventory = &snapshot.inventory;
        let slots: Vec<_> = (0..36)
            .map(|i| output(&inventory.slot(PlayerSlot::Storage(i)).cloned()))
            .collect();
        let craft: Vec<_> = (0..spec["craft"].as_array().unwrap().len() as u8)
            .map(|i| output(&inventory.slot(PlayerSlot::Crafting(i)).cloned()))
            .collect();
        let actual = json!([
            slots,
            craft,
            output(&inventory.slot(PlayerSlot::Offhand).cloned()),
            output(&inventory.cursor().cloned())
        ]);
        assert_eq!(actual, expected, "case {case}");
    }
    println!(
        "Compared {} inventory drag cases with vanilla 26.3",
        cases.len()
    );
}
