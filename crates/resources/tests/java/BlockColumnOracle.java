import java.lang.reflect.*;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.*;
import net.minecraft.util.random.WeightedList;
import net.minecraft.util.valueproviders.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.BlockColumnFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.*;
import net.minecraft.world.level.material.Fluids;

public class BlockColumnOracle {
  static BlockColumnFeature feature(int kind,Direction d,boolean tip) {
    var sugar=BlockStateProvider.of(Blocks.SUGAR_CANE);
    var stone=BlockStateProvider.of(Blocks.STONE);
    var weighted=new WeightedStateProvider(WeightedList.<BlockState>builder().add(Blocks.SUGAR_CANE.defaultBlockState(),3).add(Blocks.STONE.defaultBlockState(),1));
    var layers=switch(kind) {
      case 0 -> List.of(BlockColumnFeature.layer(ConstantInt.of(0),sugar));
      case 1 -> List.of(BlockColumnFeature.layer(BiasedToBottomInt.of(2,4),sugar));
      case 2 -> List.of(BlockColumnFeature.layer(UniformInt.of(1,3),sugar),BlockColumnFeature.layer(UniformInt.of(0,2),stone),BlockColumnFeature.layer(ConstantInt.of(1),weighted));
      default -> List.of(BlockColumnFeature.layer(BiasedToBottomInt.of(1,3),BlockStateProvider.of(Blocks.CACTUS)),BlockColumnFeature.layer(new WeightedListInt(WeightedList.<IntProvider>builder().add(ConstantInt.of(0),3).add(ConstantInt.of(1),1).build()),BlockStateProvider.of(Blocks.CACTUS_FLOWER)));
    };
    return new BlockColumnFeature(layers,d,BlockPredicate.matchesBlocks(Blocks.AIR),tip);
  }
  public static void main(String[]args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.SUPPORTS_SUGAR_CANE,BlockTags.SUPPORTS_VEGETATION));
    bind.invoke(Blocks.SAND.builtInRegistryHolder(),List.of(BlockTags.SUPPORTS_CACTUS,BlockTags.SUPPORTS_DRY_VEGETATION));
    bind.invoke(Blocks.FROSTED_ICE.builtInRegistryHolder(),List.of(BlockTags.SUPPORTS_SUGAR_CANE_ADJACENTLY));
    bind.invoke(Fluids.WATER.builtInRegistryHolder(),List.of(FluidTags.SUPPORTS_SUGAR_CANE_ADJACENTLY));
    bind.invoke(Fluids.FLOWING_WATER.builtInRegistryHolder(),List.of(FluidTags.SUPPORTS_SUGAR_CANE_ADJACENTLY));
    bind.invoke(Fluids.LAVA.builtInRegistryHolder(),List.of(FluidTags.LAVA));
    bind.invoke(Fluids.FLOWING_LAVA.builtInRegistryHolder(),List.of(FluidTags.LAVA));
    var sugar=Blocks.SUGAR_CANE.defaultBlockState();
    for(String name:List.of("oak_sapling","spruce_sapling","birch_sapling","jungle_sapling","acacia_sapling","dark_oak_sapling","cherry_sapling","pale_oak_sapling")) {
      var b=BuiltInRegistries.BLOCK.getValue(net.minecraft.resources.Identifier.parse("minecraft:"+name));if(!(b instanceof SaplingBlock))throw new AssertionError("not sapling "+name);
      for(int base=0;base<4;base++) {var state=switch(base){case 0->Blocks.STONE.defaultBlockState();case 1->Blocks.DIRT.defaultBlockState();case 2->Blocks.AIR.defaultBlockState();default->Blocks.WATER.defaultBlockState();};
        var level=(LevelReader)Proxy.newProxyInstance(LevelReader.class.getClassLoader(),new Class[]{LevelReader.class},(o,m,a)->{if(m.getName().equals("getBlockState"))return state;throw new UnsupportedOperationException(m.getName());});
        out.printf("P minecraft:%s %d %s%n",name,base,b.defaultBlockState().canSurvive(level,BlockPos.ZERO));
      }
    }
    for(String name:List.of("short_grass","fern","bush","dandelion","poppy","blue_orchid","allium","azure_bluet","red_tulip","orange_tulip","white_tulip","pink_tulip","oxeye_daisy","cornflower","lily_of_the_valley","firefly_bush","closed_eyeblossom","sweet_berry_bush","short_dry_grass","tall_dry_grass","dead_bush")) {
      var b=BuiltInRegistries.BLOCK.getValue(net.minecraft.resources.Identifier.parse("minecraft:"+name));
      for(int base=0;base<5;base++) {var state=switch(base){case 0->Blocks.STONE.defaultBlockState();case 1->Blocks.DIRT.defaultBlockState();case 2->Blocks.AIR.defaultBlockState();case 3->Blocks.WATER.defaultBlockState();default->Blocks.SAND.defaultBlockState();};
        var level=(LevelReader)Proxy.newProxyInstance(LevelReader.class.getClassLoader(),new Class[]{LevelReader.class},(o,m,a)->{if(m.getName().equals("getBlockState"))return state;throw new UnsupportedOperationException(m.getName());});
        out.printf("V minecraft:%s %d %s%n",name,base,b.defaultBlockState().canSurvive(level,BlockPos.ZERO));
      }
    }
    out.println("E "+sugar.hasBlockEntity()+" "+net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(sugar)+" "+sugar.getPostProcessPos(null,BlockPos.ZERO));
    for(var b:List.of(Blocks.CACTUS,Blocks.CACTUS_FLOWER,Blocks.SUGAR_CANE,Blocks.SAND,Blocks.STONE,Blocks.AIR,Blocks.WATER,Blocks.LAVA)) {
      var s=b.defaultBlockState();out.printf("J %s %s %s %s %s %s%n",BuiltInRegistries.BLOCK.getKey(b),s.isSolid(),s.liquid(),s.hasBlockEntity(),net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(s),s.getPostProcessPos(null,BlockPos.ZERO));
    }
    for(int base=0;base<4;base++)for(int neighbor=0;neighbor<5;neighbor++)for(int side=0;side<5;side++) {
      var map=new HashMap<BlockPos,BlockState>();
      map.put(new BlockPos(0,-1,0),switch(base){case 0->Blocks.STONE.defaultBlockState();case 1->Blocks.DIRT.defaultBlockState();case 2->sugar;default->Blocks.AIR.defaultBlockState();});
      if(side<4) {var p=BlockPos.ZERO.below().relative(new Direction[]{Direction.NORTH,Direction.EAST,Direction.SOUTH,Direction.WEST}[side]);
        var s=switch(neighbor){case 0->Blocks.AIR.defaultBlockState();case 1->Blocks.WATER.defaultBlockState();case 2->Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,3);case 3->Blocks.LAVA.defaultBlockState();default->Blocks.FROSTED_ICE.defaultBlockState();};map.put(p,s);}
      var level=(LevelReader)Proxy.newProxyInstance(LevelReader.class.getClassLoader(),new Class[]{LevelReader.class},(o,m,a)->{
        var state=map.getOrDefault((BlockPos)a[0],Blocks.AIR.defaultBlockState());return switch(m.getName()){case "getBlockState"->state;case "getFluidState"->state.getFluidState();default->throw new UnsupportedOperationException(m.getName());};
      });
      out.printf("S %d %d %d %s%n",base,neighbor,side,sugar.canSurvive(level,BlockPos.ZERO));
    }
    for(int base=0;base<3;base++)for(int neighbor=0;neighbor<7;neighbor++)for(int side=0;side<5;side++)for(int above=0;above<5;above++) {
      var map=new HashMap<BlockPos,BlockState>();
      map.put(BlockPos.ZERO.below(),switch(base){case 0->Blocks.SAND.defaultBlockState();case 1->Blocks.CACTUS.defaultBlockState();default->Blocks.DIRT.defaultBlockState();});
      map.put(BlockPos.ZERO.above(),switch(above){case 0->Blocks.AIR.defaultBlockState();case 1->Blocks.WATER.defaultBlockState();case 2->Blocks.LAVA.defaultBlockState();case 3->Blocks.STONE.defaultBlockState();default->Blocks.CACTUS_FLOWER.defaultBlockState();});
      if(side<4){var p=BlockPos.ZERO.relative(new Direction[]{Direction.NORTH,Direction.EAST,Direction.SOUTH,Direction.WEST}[side]);map.put(p,switch(neighbor){case 0->Blocks.AIR.defaultBlockState();case 1->Blocks.STONE.defaultBlockState();case 2->Blocks.CACTUS.defaultBlockState();case 3->Blocks.WATER.defaultBlockState();case 4->Blocks.LAVA.defaultBlockState();case 5->Blocks.SUGAR_CANE.defaultBlockState();default->Blocks.CACTUS_FLOWER.defaultBlockState();});}
      var level=(LevelReader)Proxy.newProxyInstance(LevelReader.class.getClassLoader(),new Class[]{LevelReader.class},(o,m,a)->{var s=map.getOrDefault((BlockPos)a[0],Blocks.AIR.defaultBlockState());return switch(m.getName()){case "getBlockState"->s;case "getFluidState"->s.getFluidState();default->throw new UnsupportedOperationException(m.getName());};});
      out.printf("K %d %d %d %d %s%n",base,neighbor,side,above,Blocks.CACTUS.defaultBlockState().canSurvive(level,BlockPos.ZERO));
    }
    int n=0;
    for(Direction d:Direction.values())for(boolean tip:new boolean[]{false,true})for(int obstacle=0;obstacle<6;obstacle++)for(int restricted=0;restricted<2;restricted++)for(int kind=0;kind<4;kind++)for(long seed:new long[]{0,7,-1}) {
      var blocked=BlockPos.ZERO.relative(d,obstacle);var changes=new LinkedHashMap<BlockPos,BlockState>();int restrict=restricted;
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
        var p=(BlockPos)a[0];var current=changes.getOrDefault(p,p.equals(blocked)?Blocks.STONE.defaultBlockState():Blocks.AIR.defaultBlockState());
        return switch(m.getName()) {case "getBlockState"->current;case "isStateAtPosition"->((Predicate<BlockState>)a[1]).test(current);case "setBlock"->{if(restrict==0)changes.put(p.immutable(),(BlockState)a[1]);yield restrict==0;}default->throw new UnsupportedOperationException(m.getName());};
      });
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      boolean result=feature(kind,d,tip).place(world,null,random,BlockPos.ZERO);
      out.printf("C %d %s %s %d %d %d %d %s %d%n",n++,d.getName(),tip,obstacle,restricted,kind,seed,result,random.nextLong());
      for(var e:changes.entrySet()){var p=e.getKey();out.printf("B %d %d %d %s%n",p.getX(),p.getY(),p.getZ(),BuiltInRegistries.BLOCK.getKey(e.getValue().getBlock()));}
    }
  }
}
