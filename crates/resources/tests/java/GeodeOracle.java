import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.feature.GeodeFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.util.valueproviders.UniformInt;

public class GeodeOracle {
 static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
 static String state(BlockState s){
  String n=net.minecraft.core.registries.BuiltInRegistries.BLOCK.getKey(s.getBlock()).getPath();
  if(s.hasProperty(BlockStateProperties.FACING))n+=":"+s.getValue(BlockStateProperties.FACING);
  if(s.hasProperty(BlockStateProperties.WATERLOGGED))n+=":"+s.getValue(BlockStateProperties.WATERLOGGED);
  return n;
 }
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var cfg=new GeodeBlockSettings(BlockStateProvider.holderOf(Blocks.AIR),BlockStateProvider.holderOf(Blocks.AMETHYST_BLOCK),BlockStateProvider.holderOf(Blocks.BUDDING_AMETHYST),BlockStateProvider.holderOf(Blocks.CALCITE),BlockStateProvider.holderOf(Blocks.SMOOTH_BASALT),List.of(Blocks.SMALL_AMETHYST_BUD.defaultBlockState(),Blocks.MEDIUM_AMETHYST_BUD.defaultBlockState(),Blocks.LARGE_AMETHYST_BUD.defaultBlockState(),Blocks.AMETHYST_CLUSTER.defaultBlockState()),HolderSet.direct(Blocks.BEDROCK.builtInRegistryHolder()),HolderSet.direct(Blocks.BEDROCK.builtInRegistryHolder(),Blocks.WATER.builtInRegistryHolder(),Blocks.LAVA.builtInRegistryHolder(),Blocks.ICE.builtInRegistryHolder(),Blocks.PACKED_ICE.builtInRegistryHolder(),Blocks.BLUE_ICE.builtInRegistryHolder()));
  var feature=new GeodeFeature(cfg,new GeodeLayerSettings(1.7,2.2,3.2,4.2),new GeodeCrackSettings(.95,2,2),.35,.083,true,UniformInt.of(4,6),UniformInt.of(3,4),UniformInt.of(1,2),-16,16,.05,1);
  for(long seed:new long[]{0,1,-1,987654321})for(int mode=0;mode<3;mode++){
   final int scenario=mode;var blocks=new HashMap<BlockPos,BlockState>();var writes=new ArrayList<String>();var ticks=new ArrayList<String>();
   var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
    if(m.getName().equals("getSeed"))return seed+123;
    if(m.getName().equals("getBlockState")||m.getName().equals("getFluidState")){
     var p=(BlockPos)v[0];BlockState original=scenario==2?Blocks.AIR.defaultBlockState():scenario==1&&Math.floorMod(p.getX()+p.getY()+p.getZ(),17)==0?Blocks.BEDROCK.defaultBlockState():scenario==1&&Math.floorMod(p.getX()*3+p.getY()+p.getZ(),23)==0?Blocks.WATER.defaultBlockState():Blocks.STONE.defaultBlockState();
     var s=blocks.getOrDefault(p,original);return m.getName().equals("getFluidState")?s.getFluidState():s;
    }
    if(m.getName().equals("setBlock")){var p=((BlockPos)v[0]).immutable();var s=(BlockState)v[1];if((int)v[2]!=2)throw new AssertionError("flags");blocks.put(p,s);writes.add(pos(p)+","+state(s));return true;}
    if(m.getName().equals("scheduleTick")){ticks.add(pos((BlockPos)v[0]));return null;}
    throw new UnsupportedOperationException(m.toString());
   });
   var random=new WorldgenRandom(new XoroshiroRandomSource(seed));boolean ok=feature.place(world,null,random,new BlockPos(0,20,0));
   out.printf("C %d %d %s W %s T %s R %d%n",seed,mode,ok,String.join(";",writes),String.join(";",ticks),random.nextLong());
  }
 }
}
