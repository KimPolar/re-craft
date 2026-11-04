import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.RandomSequence;
import net.minecraft.world.item.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.storage.loot.*;
import net.minecraft.world.level.storage.loot.parameters.LootContextParams;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.core.BlockPos;
import net.minecraft.server.level.ServerLevel;

public class LeafDecayLootOracle {
  static class World extends net.minecraft.client.multiplayer.ClientLevel {
    net.minecraft.util.RandomSource seeded;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public net.minecraft.util.RandomSource getRandom() { return seeded; }
  }
  static BlockState current, replacement;
  static class Server extends ServerLevel {
    Server() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
    @Override public FluidState getFluidState(BlockPos p) { return current.getFluidState(); }
    @Override public boolean setBlock(BlockPos p, BlockState state, int flags) { replacement=state; return true; }
  }
  static JsonElement expand(JsonElement value, JsonElement shear, JsonElement silk) {
    if(value.isJsonPrimitive() && value.getAsJsonPrimitive().isString()) {
      if(value.getAsString().equals("minecraft:tool/can_shear")) return shear.deepCopy();
      if(value.getAsString().equals("minecraft:tool/can_silk_touch")) return silk.deepCopy();
    }
    if(value.isJsonArray()) { var a = new JsonArray(); for(var item:value.getAsJsonArray()) a.add(expand(item,shear,silk)); return a; }
    if(value.isJsonObject()) { var o = new JsonObject(); for(var entry:value.getAsJsonObject().entrySet()) o.add(entry.getKey(),expand(entry.getValue(),shear,silk)); return o; }
    return value.deepCopy();
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registries=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var ops=RegistryOps.create(JsonOps.INSTANCE,registries);
    var shear=JsonParser.parseString(Files.readString(Path.of(args[0])));
    var silk=JsonParser.parseString(Files.readString(Path.of(args[1])));
    var ctor=LootContext.class.getDeclaredConstructors()[0]; ctor.setAccessible(true);
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
    var leaves=List.of(Blocks.OAK_LEAVES,Blocks.SPRUCE_LEAVES,Blocks.BIRCH_LEAVES,Blocks.JUNGLE_LEAVES,
        Blocks.ACACIA_LEAVES,Blocks.DARK_OAK_LEAVES,Blocks.MANGROVE_LEAVES,Blocks.CHERRY_LEAVES,
        Blocks.PALE_OAK_LEAVES,Blocks.AZALEA_LEAVES,Blocks.FLOWERING_AZALEA_LEAVES);
    var result=new JsonArray();
    for(int i=0;i<leaves.size();i++) {
      var block=leaves.get(i);
      var name=BuiltInRegistries.BLOCK.getKey(block).getPath();
      var json=expand(JsonParser.parseString(Files.readString(Path.of(args[i+2]))),shear,silk);
      var table=LootTable.DIRECT_CODEC.parse(ops,json).getOrThrow();
      for(long seed:new long[]{0,1,42,-17,123456789}) for(int step=0;step<16;step++) {
        var random=new RandomSequence(seed,Identifier.parse("minecraft:blocks/"+name)).random();
        var world=(World)unsafe.allocateInstance(World.class); world.seeded=new net.minecraft.world.level.levelgen.LegacyRandomSource(9182);
        var params=new LootParams(null,ContextMap.builder().set(LootContextParams.TOOL,ItemStack.EMPTY).set(LootContextParams.BLOCK_STATE,block.defaultBlockState()).build(),Map.of(),0.0f);
        List<ItemStack> items=List.of();
        for(int n=0;n<=step;n++) {
          var context=(LootContext)ctor.newInstance(params,random,registries);
          var next=new ArrayList<ItemStack>(); table.getRandomItemsRaw(context,next::add); items=next;
          for(var item:items) if(!item.isEmpty()) Block.popResource(world,net.minecraft.core.BlockPos.ZERO,item);
        }
        var drops=new JsonArray(); for(var stack:items) if(!stack.isEmpty()) {
          var item=new JsonArray(); item.add(BuiltInRegistries.ITEM.getKey(stack.getItem()).toString()); item.add(stack.getCount()); drops.add(item);
        }
        var row=new JsonObject(); row.addProperty("name",name); row.addProperty("seed",seed); row.addProperty("step",step); row.add("drops",drops); row.addProperty("next",random.nextLong()); row.addProperty("levelNext",world.seeded.nextDouble()); result.add(row);
      }
    }
    Files.writeString(Path.of(args[13]),result.toString());
    var states=new JsonArray();
    var server=(Server)unsafe.allocateInstance(Server.class);
    for(var block:leaves) for(int distance=1;distance<=7;distance++) for(boolean persistent:new boolean[]{false,true}) for(boolean wet:new boolean[]{false,true}) {
      current=block.defaultBlockState().setValue(LeavesBlock.DISTANCE,distance).setValue(LeavesBlock.PERSISTENT,persistent).setValue(LeavesBlock.WATERLOGGED,wet);
      server.removeBlock(BlockPos.ZERO,false);
      var row=new JsonObject(); row.addProperty("name",BuiltInRegistries.BLOCK.getKey(block).toString());
      row.addProperty("distance",distance); row.addProperty("persistent",persistent); row.addProperty("wet",wet);
      row.addProperty("ticks",current.isRandomlyTicking()); row.addProperty("replacement",BuiltInRegistries.BLOCK.getKey(replacement.getBlock()).toString());
      states.add(row);
    }
    Files.writeString(Path.of(args[14]),states.toString());
  }
}
