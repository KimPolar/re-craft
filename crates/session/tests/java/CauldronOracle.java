import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.core.*;
import net.minecraft.core.cauldron.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.level.*;
import net.minecraft.sounds.*;
import net.minecraft.stats.Stat;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.*;
import net.minecraft.world.item.alchemy.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.phys.Vec3;

public class CauldronOracle {
  static class World extends ServerLevel {
    BlockState state; String sound, category; boolean aboveWater;
    World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
    @Override public boolean setBlockAndUpdate(BlockPos p, BlockState s) { state=s; return true; }
    @Override public net.minecraft.world.level.material.FluidState getFluidState(BlockPos p) { return (aboveWater?net.minecraft.world.level.material.Fluids.WATER:net.minecraft.world.level.material.Fluids.EMPTY).defaultFluidState(); }
    @Override public void playSound(Entity e, BlockPos p, SoundEvent s, SoundSource c, float v, float pitch) { sound=s.location().toString(); category=c.toString(); }
    @Override public void gameEvent(Holder<GameEvent> event, Vec3 pos, GameEvent.Context context) {}
  }
  static class Player extends ServerPlayer {
    Inventory inventory; boolean creative;
    Player() { super(null,null,null,null); }
    @Override public Inventory getInventory() { return inventory; }
    @Override public boolean hasInfiniteMaterials() { return creative; }
    @Override public void setItemInHand(InteractionHand hand, ItemStack value) { inventory.setItem(hand==InteractionHand.MAIN_HAND?0:40,value); }
    @Override public void onEquipItem(EquipmentSlot s, ItemStack before, ItemStack after) {}
    @Override public void awardStat(Identifier id) {}
    @Override public void awardStat(Stat<?> stat) {}
  }
  static String describe(ItemStack s) { return s.isEmpty()?"-":BuiltInRegistries.ITEM.getKey(s.getItem()).toString(); }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    ((MappedRegistry<net.minecraft.world.level.material.Fluid>)BuiltInRegistries.FLUID).prepareTagReload(new net.minecraft.tags.TagLoader.LoadResult<>(net.minecraft.core.registries.Registries.FLUID,Map.of(net.minecraft.tags.FluidTags.WATER,List.of(net.minecraft.world.level.material.Fluids.WATER.builtInRegistryHolder())))).apply();
    if (!net.minecraft.world.level.material.Fluids.WATER.defaultFluidState().is(net.minecraft.tags.FluidTags.WATER)) throw new AssertionError("water tag was not bound");
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var player=(Player)unsafe.allocateInstance(Player.class);
    var out=new JsonArray();
    for(int initial=0;initial<=5;initial++) for(int item=0;item<5;item++) for(boolean creative:new boolean[]{false,true}) for(boolean off:new boolean[]{false,true}) for(boolean aboveWater:new boolean[]{false,true}) {
      world.state=switch(initial) { case 0 -> Blocks.CAULDRON.defaultBlockState(); case 1,2,3 -> Blocks.WATER_CAULDRON.defaultBlockState().setValue(LayeredCauldronBlock.LEVEL,initial); case 4 -> Blocks.LAVA_CAULDRON.defaultBlockState(); default -> Blocks.POWDER_SNOW_CAULDRON.defaultBlockState(); };
      var stack=switch(item) { case 0 -> new ItemStack(Items.GLASS_BOTTLE); case 1 -> PotionContents.createItemStack(Items.POTION,Potions.WATER); case 2 -> new ItemStack(Items.BUCKET); case 3 -> new ItemStack(Items.WATER_BUCKET); default -> new ItemStack(Items.LAVA_BUCKET); };
      player.creative=creative; player.inventory=new Inventory(player,new EntityEquipment()); player.inventory.setItem(off?40:0,stack); world.sound=null; world.category=null; world.aboveWater=aboveWater;
      var dispatch=initial==0?CauldronInteractions.EMPTY:initial<=3?CauldronInteractions.WATER:initial==4?CauldronInteractions.LAVA:CauldronInteractions.POWDER_SNOW;
      var result=dispatch.get(stack).interact(world.state,world,BlockPos.ZERO,player,off?InteractionHand.OFF_HAND:InteractionHand.MAIN_HAND,stack);
      var row=new JsonObject(); row.addProperty("initial",initial); row.addProperty("item",item); row.addProperty("creative",creative); row.addProperty("off",off); row.addProperty("above_water",aboveWater); row.addProperty("handled",result.consumesAction()); row.addProperty("block",BuiltInRegistries.BLOCK.getKey(world.state.getBlock()).toString());
      row.addProperty("level",world.state.hasProperty(LayeredCauldronBlock.LEVEL)?world.state.getValue(LayeredCauldronBlock.LEVEL):0); row.addProperty("hand",describe(player.inventory.getItem(off?40:0))); row.addProperty("slot1",describe(player.inventory.getItem(off?0:1))); row.addProperty("sound",world.sound); row.addProperty("category",world.category); out.add(row);
    }
    Files.writeString(Path.of(args[0]),out.toString());
  }
}
