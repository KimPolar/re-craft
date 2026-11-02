import java.lang.reflect.*;
import java.util.*;
import java.util.function.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.util.valueproviders.ConstantInt;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.TreeFeature;
import net.minecraft.world.level.levelgen.feature.featuresize.ThreeLayersFeatureSize;
import net.minecraft.world.level.levelgen.feature.foliageplacers.DarkOakFoliagePlacer;
import net.minecraft.world.level.levelgen.feature.foliageplacers.FoliagePlacer;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.trunkplacers.DarkOakTrunkPlacer;

public class DarkOakComponentsOracle {
  @SuppressWarnings("unchecked") public static void main(String[] args)throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
    bind.invoke(Blocks.DARK_OAK_LOG.builtInRegistryHolder(),List.of(BlockTags.LOGS));
    bind.invoke(Blocks.DARK_OAK_LEAVES.builtInRegistryHolder(),List.of(BlockTags.REPLACEABLE_BY_TREES,BlockTags.LEAVES));
    for(var b:List.of(Blocks.VINE,Blocks.WATER)) bind.invoke(b.builtInRegistryHolder(),List.of(BlockTags.REPLACEABLE_BY_TREES));
    var method=TreeFeature.class.getDeclaredMethod("doPlace",WorldGenLevel.class,RandomSource.class,BlockPos.class,BiConsumer.class,BiConsumer.class,FoliagePlacer.FoliageSetter.class); method.setAccessible(true);
    for(long seed:new long[]{0,1,-1,2,42,123,987654321,Long.MIN_VALUE}) for(int mode=0;mode<6;mode++) for(boolean ignore:new boolean[]{false,true}) {
      var blocks=new HashMap<BlockPos,BlockState>(); var writes=new ArrayList<String>();
      for(int x=3;x<=4;x++)for(int z=-4;z<=-3;z++)blocks.put(new BlockPos(x,2,z),Blocks.STONE.defaultBlockState());
      if(mode==1)blocks.put(new BlockPos(3,5,-4),Blocks.STONE.defaultBlockState());
      if(mode==2)blocks.put(new BlockPos(4,6,-4),Blocks.DARK_OAK_LEAVES.defaultBlockState().setValue(BlockStateProperties.PERSISTENT,true));
      if(mode==3)blocks.put(new BlockPos(2,6,-4),Blocks.WATER.defaultBlockState());
      if(mode==4)blocks.put(new BlockPos(3,5,-4),Blocks.VINE.defaultBlockState());
      if(mode==5)blocks.put(new BlockPos(3,5,-4),Blocks.DARK_OAK_LOG.defaultBlockState());
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
        if(m.getName().equals("getMinY"))return 0;if(m.getName().equals("getMaxY"))return 40;
        if(m.getName().equals("isStateAtPosition"))return ((Predicate<BlockState>)v[1]).test(blocks.getOrDefault(v[0],Blocks.AIR.defaultBlockState()));
        if(m.getName().equals("isFluidAtPosition")){var state=blocks.getOrDefault(v[0],Blocks.AIR.defaultBlockState());return ((Predicate<FluidState>)v[1]).test(state.is(Blocks.WATER)?Fluids.WATER.defaultFluidState():Fluids.EMPTY.defaultFluidState());}
        throw new UnsupportedOperationException(m.toString());
      });
      BiConsumer<BlockPos,BlockState> trunk=(p,s)->{p=p.immutable();blocks.put(p,s);writes.add("T"+p.getX()+","+p.getY()+","+p.getZ()+","+BuiltInRegistries.BLOCK.getKey(s.getBlock()));};
      var foliage=new FoliagePlacer.FoliageSetter(){public void set(BlockPos p,BlockState s){p=p.immutable();blocks.put(p,s);writes.add("L"+p.getX()+","+p.getY()+","+p.getZ()+","+BuiltInRegistries.BLOCK.getKey(s.getBlock())+","+s.getValue(BlockStateProperties.WATERLOGGED));}public boolean isSet(BlockPos p){return false;}};
      var tree=new TreeFeature(BlockStateProvider.holderOf(Blocks.DARK_OAK_LOG),new DarkOakTrunkPlacer(6,2,1),BlockStateProvider.holderOf(Blocks.DARK_OAK_LEAVES),new DarkOakFoliagePlacer(ConstantInt.of(0),ConstantInt.of(0)),Optional.empty(),new ThreeLayersFeatureSize(1,1,0,1,2,OptionalInt.empty()),List.of(),ignore,BlockStateProvider.holderOf(Blocks.DIRT));
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      boolean ok=(boolean)method.invoke(tree,world,random,new BlockPos(3,3,-4),trunk,trunk,foliage);
      out.printf("D %d %d %s %s %s R %d%n",seed,mode,ignore,ok,String.join(";",writes),random.nextLong());
    }
  }
}
