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
import net.minecraft.util.valueproviders.UniformInt;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.TreeFeature;
import net.minecraft.world.level.levelgen.feature.featuresize.TwoLayersFeatureSize;
import net.minecraft.world.level.levelgen.feature.foliageplacers.RandomSpreadFoliagePlacer;
import net.minecraft.world.level.levelgen.feature.foliageplacers.FoliagePlacer;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.trunkplacers.BendingTrunkPlacer;

public class BendingTreeComponentsOracle {
  @SuppressWarnings("unchecked") public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.OAK_LOG.builtInRegistryHolder(),List.of(BlockTags.LOGS));
    for(var b:List.of(Blocks.OAK_LEAVES,Blocks.VINE,Blocks.WATER))bind.invoke(b.builtInRegistryHolder(),List.of(BlockTags.REPLACEABLE_BY_TREES));
    var method=TreeFeature.class.getDeclaredMethod("doPlace",WorldGenLevel.class,RandomSource.class,BlockPos.class,BiConsumer.class,BiConsumer.class,FoliagePlacer.FoliageSetter.class);method.setAccessible(true);
    for(long seed:new long[]{0,1,-1,42,987654321,Long.MIN_VALUE})for(int mode=0;mode<6;mode++)for(boolean ignore:new boolean[]{false,true}) {
      var blocks=new HashMap<BlockPos,BlockState>();var writes=new ArrayList<String>();
      blocks.put(new BlockPos(3,2,-4),Blocks.STONE.defaultBlockState());
      if(mode==1)blocks.put(new BlockPos(3,5,-4),Blocks.STONE.defaultBlockState());
      if(mode==2)blocks.put(new BlockPos(4,6,-4),Blocks.OAK_LEAVES.defaultBlockState().setValue(BlockStateProperties.PERSISTENT,true));
      if(mode==3)blocks.put(new BlockPos(2,6,-4),Blocks.WATER.defaultBlockState());
      if(mode==4)blocks.put(new BlockPos(3,5,-4),Blocks.VINE.defaultBlockState());
      if(mode==5)blocks.put(new BlockPos(3,5,-4),Blocks.OAK_LOG.defaultBlockState());
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
        if(m.getName().equals("getMinY"))return 0;if(m.getName().equals("getMaxY"))return 40;
        if(m.getName().equals("isStateAtPosition"))return ((Predicate<BlockState>)v[1]).test(blocks.getOrDefault(v[0],Blocks.AIR.defaultBlockState()));
        if(m.getName().equals("isFluidAtPosition")){var state=blocks.getOrDefault(v[0],Blocks.AIR.defaultBlockState());return ((Predicate<FluidState>)v[1]).test(state.is(Blocks.WATER)?Fluids.WATER.defaultFluidState():Fluids.EMPTY.defaultFluidState());}
        throw new UnsupportedOperationException(m.toString());
      });
      BiConsumer<BlockPos,BlockState> trunk=(p,s)->{p=p.immutable();blocks.put(p,s);writes.add("T"+p.getX()+","+p.getY()+","+p.getZ()+","+BuiltInRegistries.BLOCK.getKey(s.getBlock()));};
      var foliage=new FoliagePlacer.FoliageSetter(){public void set(BlockPos p,BlockState s){p=p.immutable();blocks.put(p,s);writes.add("L"+p.getX()+","+p.getY()+","+p.getZ()+","+BuiltInRegistries.BLOCK.getKey(s.getBlock())+","+s.getValue(BlockStateProperties.WATERLOGGED));}public boolean isSet(BlockPos p){return false;}};
      var tree=new TreeFeature(BlockStateProvider.holderOf(Blocks.OAK_LOG),new BendingTrunkPlacer(4,2,0,3,UniformInt.of(1,2)),BlockStateProvider.holderOf(Blocks.OAK_LEAVES),new RandomSpreadFoliagePlacer(UniformInt.of(2,3),UniformInt.of(0,1),UniformInt.of(2,4),50),Optional.empty(),new TwoLayersFeatureSize(1,0,1),List.of(),ignore,BlockStateProvider.holderOf(Blocks.DIRT));
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      boolean ok=(boolean)method.invoke(tree,world,random,new BlockPos(3,3,-4),trunk,trunk,foliage);
      out.printf("B %d %d %s %s %s R %d%n",seed,mode,ignore,ok,String.join(";",writes),random.nextLong());
    }
  }
}
