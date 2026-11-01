import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.core.*;
import net.minecraft.core.cauldron.CauldronInteractions;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.EntityEquipment;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.DyedItemColor;
import net.minecraft.world.level.block.*;

public class CauldronDyeOracle {
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    if(new ItemStack(Items.LEATHER_HORSE_ARMOR).getMaxStackSize()!=1 || new ItemStack(Items.WOLF_ARMOR).getMaxStackSize()!=1 || new ItemStack(Items.WOLF_ARMOR).getOrDefault(DataComponents.MAX_DAMAGE,0)!=64) throw new AssertionError("animal armor prototypes differ");
    ((MappedRegistry<Item>)BuiltInRegistries.ITEM).prepareTagReload(new TagLoader.LoadResult<>(Registries.ITEM,Map.of(ItemTags.CAULDRON_CAN_REMOVE_DYE,List.of(Items.LEATHER_CHESTPLATE.builtInRegistryHolder(),Items.LEATHER_HORSE_ARMOR.builtInRegistryHolder(),Items.WOLF_ARMOR.builtInRegistryHolder(),Items.STONE.builtInRegistryHolder(),Items.GLASS_BOTTLE.builtInRegistryHolder(),Items.WATER_BUCKET.builtInRegistryHolder(),Items.POTION.builtInRegistryHolder())))).apply();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(CauldronOracle.World)unsafe.allocateInstance(CauldronOracle.World.class); var player=(CauldronOracle.Player)unsafe.allocateInstance(CauldronOracle.Player.class);
    var defaults=new JsonArray(); for(var item:BuiltInRegistries.ITEM) if(new ItemStack(item).has(DataComponents.DYED_COLOR)) defaults.add(BuiltInRegistries.ITEM.getKey(item).toString());
    var rows=new JsonArray();
    for(var item:new Item[]{Items.LEATHER_CHESTPLATE,Items.LEATHER_HORSE_ARMOR,Items.WOLF_ARMOR,Items.STONE,Items.IRON_CHESTPLATE,Items.GLASS_BOTTLE,Items.WATER_BUCKET,Items.POTION}) for(int level=1;level<=3;level++) for(int dye:new int[]{-1,0,0x123abc}) for(boolean creative:new boolean[]{false,true}) {
      world.state=Blocks.WATER_CAULDRON.defaultBlockState().setValue(LayeredCauldronBlock.LEVEL,level); world.sound=null; world.aboveWater=false;
      player.creative=creative; player.inventory=new Inventory(player,new EntityEquipment()); var stack=new ItemStack(item); if(dye>=0) stack.set(DataComponents.DYED_COLOR,new DyedItemColor(dye));
      if(stack.has(DataComponents.MAX_DAMAGE)) stack.set(DataComponents.DAMAGE,5); player.inventory.setItem(0,stack);
      var result=CauldronInteractions.WATER.get(stack).interact(world.state,world,BlockPos.ZERO,player,InteractionHand.MAIN_HAND,stack);
      var row=new JsonObject(); row.addProperty("item",BuiltInRegistries.ITEM.getKey(item).toString()); row.addProperty("level",level); row.addProperty("dye",dye); row.addProperty("creative",creative); row.addProperty("handled",result.consumesAction()); row.addProperty("block",BuiltInRegistries.BLOCK.getKey(world.state.getBlock()).toString()); row.addProperty("remaining",world.state.hasProperty(LayeredCauldronBlock.LEVEL)?world.state.getValue(LayeredCauldronBlock.LEVEL):0); row.addProperty("has_dye",stack.has(DataComponents.DYED_COLOR)); row.addProperty("damage",stack.getOrDefault(DataComponents.DAMAGE,-1)); row.addProperty("sound",world.sound); rows.add(row);
    }
    var output=new JsonObject(); output.add("defaults",defaults); output.add("rows",rows); Files.writeString(Path.of(args[0]),output.toString());
  }
}
