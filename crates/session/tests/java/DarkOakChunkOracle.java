import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.valueproviders.ConstantInt;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Block;
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
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.trunkplacers.DarkOakTrunkPlacer;

public class DarkOakChunkOracle {
  static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
  @SuppressWarnings("unchecked") public static void main(String[] args)throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.DARK_OAK_LOG.builtInRegistryHolder(),List.of(BlockTags.LOGS,BlockTags.PREVENTS_NEARBY_LEAF_DECAY));
    bind.invoke(Blocks.DARK_OAK_LEAVES.builtInRegistryHolder(),List.of(BlockTags.REPLACEABLE_BY_TREES,BlockTags.LEAVES));
    var blocks=new HashMap<BlockPos,BlockState>(); var ticks=new LinkedHashSet<String>();
    var worldRandom=new WorldgenRandom(new XoroshiroRandomSource(123));
    var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
      if(m.getName().equals("getMinY"))return 0;if(m.getName().equals("getMaxY"))return 15;
      if(m.getName().equals("getRandom"))return worldRandom;
      if(m.getName().equals("getBlockState")||m.getName().equals("isStateAtPosition")){
        var p=(BlockPos)v[0];var s=blocks.getOrDefault(p,(p.getY()<0||p.getY()>15?Blocks.VOID_AIR:p.getY()<5?Blocks.STONE:Blocks.AIR).defaultBlockState());
        if(m.getName().equals("getBlockState"))return s;return ((Predicate<BlockState>)v[1]).test(s);
      }
      if(m.getName().equals("isFluidAtPosition"))return ((Predicate<FluidState>)v[1]).test(Fluids.EMPTY.defaultFluidState());
      if(m.getName().equals("setBlock")){var p=((BlockPos)v[0]).immutable();if(p.getY()>=0&&p.getY()<16)blocks.put(p,(BlockState)v[1]);return true;}
      if(m.getName().equals("scheduleTick")){if(v[1] instanceof Block b)ticks.add(pos((BlockPos)v[0])+","+BuiltInRegistries.BLOCK.getKey(b));else throw new AssertionError("unexpected fluid tick");return null;}
      throw new UnsupportedOperationException(m.toString());
    });
    var tree=new TreeFeature(BlockStateProvider.holderOf(Blocks.DARK_OAK_LOG),new DarkOakTrunkPlacer(6,2,1),BlockStateProvider.holderOf(Blocks.DARK_OAK_LEAVES),new DarkOakFoliagePlacer(ConstantInt.of(0),ConstantInt.of(0)),Optional.empty(),new ThreeLayersFeatureSize(1,1,0,1,2,OptionalInt.empty()),List.of(),true,BlockStateProvider.holderOf(Blocks.DIRT));
    var random=new WorldgenRandom(new XoroshiroRandomSource(0));long decoration=random.setDecorationSeed(0,0,0);random.setFeatureSeed(decoration,0,0);
    boolean success=tree.place(world,null,random,new BlockPos(8,5,8));
    var result=new ArrayList<String>();
    for(int x=0;x<16;x++)for(int y=4;y<16;y++)for(int z=0;z<16;z++){
      var p=new BlockPos(x,y,z);var s=blocks.get(p);if(s==null||s.is(Blocks.AIR))continue;
      var name=BuiltInRegistries.BLOCK.getKey(s.getBlock()).toString();
      var extra=s.is(Blocks.DARK_OAK_LOG)?",axis="+s.getValue(BlockStateProperties.AXIS):s.is(Blocks.DARK_OAK_LEAVES)?",distance="+s.getValue(BlockStateProperties.DISTANCE)+",persistent="+s.getValue(BlockStateProperties.PERSISTENT)+",waterlogged="+s.getValue(BlockStateProperties.WATERLOGGED):"";
      result.add(pos(p)+","+name+extra);
    }
    out.println("RESULT "+success+" B "+String.join(";",result)+" T "+String.join(";",ticks));
  }
}
