import java.lang.reflect.*;
import java.util.*;
import java.util.function.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.util.valueproviders.UniformInt;
import net.minecraft.util.random.WeightedList;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.FallenTreeFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.stateproviders.WeightedStateProvider;
import net.minecraft.world.level.levelgen.feature.treedecorators.*;

public class FallenTreeChunkOracle {
 @SuppressWarnings("unchecked") public static void main(String[] args){
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  for(long seed:new long[]{0,1,-1,987654321,11,42,100,218}){
   var blocks=new HashMap<BlockPos,BlockState>();
   var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
    if(m.getName().equals("isStateAtPosition")||m.getName().equals("getBlockState")){
      var p=(BlockPos)v[0];var s=blocks.getOrDefault(p,(p.getY()<0||p.getY()>15?Blocks.VOID_AIR:p.getY()<8?Blocks.STONE:Blocks.AIR).defaultBlockState());
      if(m.getName().equals("getBlockState"))return s;return ((Predicate<BlockState>)v[1]).test(s);
    }
    if(m.getName().equals("setBlockAndUpdate")||m.getName().equals("setBlock")){var p=((BlockPos)v[0]).immutable();if(p.getY()>=0&&p.getY()<16)blocks.put(p,(BlockState)v[1]);return true;}
    throw new UnsupportedOperationException(m.toString());
   });
   var mushroom=new WeightedStateProvider(WeightedList.<BlockState>builder().add(Blocks.RED_MUSHROOM.defaultBlockState(),2).add(Blocks.BROWN_MUSHROOM.defaultBlockState(),1));
   var feature=new FallenTreeFeature(BlockStateProvider.holderOf(Blocks.OAK_LOG),UniformInt.of(4,7),List.of(TrunkVineDecorator.INSTANCE),List.of(new AttachedToLogsDecorator(0.1f,Holder.direct(mushroom),List.of(Direction.UP))));
   var random=new WorldgenRandom(new XoroshiroRandomSource(seed));boolean ok=feature.place(world,null,random,new BlockPos(8,8,8));
   var result=new ArrayList<String>();
   for(int x=0;x<16;x++)for(int y=8;y<16;y++)for(int z=0;z<16;z++){
    var p=new BlockPos(x,y,z);var s=blocks.get(p);if(s==null||s.is(Blocks.AIR))continue;
    String name=s.is(Blocks.OAK_LOG)?"oak_log":s.is(Blocks.VINE)?"vine":s.is(Blocks.RED_MUSHROOM)?"red_mushroom":s.is(Blocks.BROWN_MUSHROOM)?"brown_mushroom":"unexpected";
    String extra=s.is(Blocks.OAK_LOG)?","+s.getValue(BlockStateProperties.AXIS):s.is(Blocks.VINE)?","+s.getValue(BlockStateProperties.UP)+","+s.getValue(BlockStateProperties.NORTH)+","+s.getValue(BlockStateProperties.SOUTH)+","+s.getValue(BlockStateProperties.WEST)+","+s.getValue(BlockStateProperties.EAST):"";
    result.add(x+","+y+","+z+","+name+extra);
   }
   out.printf("C %d %s B %s R %d%n",seed,ok,String.join(";",result),random.nextLong());
  }
 }
}
