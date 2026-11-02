import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
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
import net.minecraft.world.level.levelgen.feature.featuresize.TwoLayersFeatureSize;
import net.minecraft.world.level.levelgen.feature.foliageplacers.BlobFoliagePlacer;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.trunkplacers.StraightTrunkPlacer;

public class TreeChunkOracle {
  static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
  @SuppressWarnings("unchecked") public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.OAK_LOG.builtInRegistryHolder(),List.of(BlockTags.LOGS,BlockTags.PREVENTS_NEARBY_LEAF_DECAY));
    bind.invoke(Blocks.OAK_LEAVES.builtInRegistryHolder(),List.of(BlockTags.REPLACEABLE_BY_TREES));
    var blocks=new HashMap<BlockPos,BlockState>();var ticks=new LinkedHashSet<String>();
    if(args.length>0 && args[0].equals("grass"))blocks.put(new BlockPos(8,8,9),Blocks.GRASS_BLOCK.defaultBlockState().setValue(BlockStateProperties.SNOWY,true));
    if(args.length>0 && args[0].startsWith("vegetation")){
      blocks.put(new BlockPos(8,8,9),Blocks.SHORT_GRASS.defaultBlockState());
      if(args[0].equals("vegetation-supported"))bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.SUPPORTS_VEGETATION));
    }
    if(args.length>0 && args[0].startsWith("sapling")){
      blocks.put(new BlockPos(8,8,9),Blocks.OAK_SAPLING.defaultBlockState().setValue(BlockStateProperties.STAGE,1));
      if(args[0].equals("sapling-supported"))bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.SUPPORTS_VEGETATION));
    }
    var worldRandom=new WorldgenRandom(new XoroshiroRandomSource(123));
    var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
      if(m.getName().equals("getMinY"))return 0;if(m.getName().equals("getMaxY"))return 15;
      if(m.getName().equals("getRandom"))return worldRandom;
      if(m.getName().equals("getBlockState")||m.getName().equals("isStateAtPosition")){
        var p=(BlockPos)v[0];var s=blocks.getOrDefault(p,(p.getY()<0||p.getY()>15?Blocks.VOID_AIR:p.getY()<8?Blocks.STONE:Blocks.AIR).defaultBlockState());
        if(m.getName().equals("getBlockState"))return s;return ((Predicate<BlockState>)v[1]).test(s);
      }
      if(m.getName().equals("isFluidAtPosition"))return ((Predicate<FluidState>)v[1]).test(Fluids.EMPTY.defaultFluidState());
      if(m.getName().equals("setBlock")){var p=((BlockPos)v[0]).immutable();if(p.getY()>=0&&p.getY()<16)blocks.put(p,(BlockState)v[1]);return true;}
      if(m.getName().equals("scheduleTick")){if(v[1] instanceof Block block)ticks.add(pos((BlockPos)v[0])+","+BuiltInRegistries.BLOCK.getKey(block));else throw new AssertionError("unexpected fluid tick");return null;}
      throw new UnsupportedOperationException(m.toString());
    });
    var tree=new TreeFeature(BlockStateProvider.holderOf(Blocks.OAK_LOG),new StraightTrunkPlacer(4,2,0),BlockStateProvider.holderOf(Blocks.OAK_LEAVES),new BlobFoliagePlacer(ConstantInt.of(2),ConstantInt.of(0),3),Optional.empty(),new TwoLayersFeatureSize(1,0,1),List.of(),true,BlockStateProvider.holderOf(Blocks.DIRT));
    var random=new WorldgenRandom(new XoroshiroRandomSource(0));long decoration=random.setDecorationSeed(0,0,0);random.setFeatureSeed(decoration,0,0);
    boolean success=tree.place(world,null,random,new BlockPos(8,8,8));
    var result=new ArrayList<String>();
    for(int x=0;x<16;x++)for(int y=7;y<16;y++)for(int z=0;z<16;z++){
      var p=new BlockPos(x,y,z);var s=blocks.get(p);if(s==null || s.is(Blocks.AIR))continue;
      var name=BuiltInRegistries.BLOCK.getKey(s.getBlock()).toString();
      var extra=s.is(Blocks.OAK_LOG)?",axis="+s.getValue(BlockStateProperties.AXIS):s.is(Blocks.OAK_LEAVES)?",distance="+s.getValue(BlockStateProperties.DISTANCE)+",persistent="+s.getValue(BlockStateProperties.PERSISTENT)+",waterlogged="+s.getValue(BlockStateProperties.WATERLOGGED):s.is(Blocks.GRASS_BLOCK)?",snowy="+s.getValue(BlockStateProperties.SNOWY):s.is(Blocks.OAK_SAPLING)?",stage="+s.getValue(BlockStateProperties.STAGE):"";
      result.add(pos(p)+","+name+extra);
    }
    out.println("RESULT "+success+" B "+String.join(";",result)+" T "+String.join(";",ticks));
    for(boolean snowyTag:new boolean[]{false,true})for(boolean initial:new boolean[]{false,true})for(var direction:List.of(Direction.UP,Direction.EAST)){
      bind.invoke(Blocks.STONE.builtInRegistryHolder(),snowyTag?List.of(BlockTags.SNOW):List.of());
      var state=Blocks.GRASS_BLOCK.defaultBlockState().setValue(BlockStateProperties.SNOWY,initial);
      var changed=state.updateShape(world,world,new BlockPos(1,8,1),direction,new BlockPos(1,9,1),Blocks.STONE.defaultBlockState(),worldRandom);
      out.printf("GRASS %s %s %s %s%n",snowyTag,initial,direction,changed.getValue(BlockStateProperties.SNOWY));
    }
  }
}
