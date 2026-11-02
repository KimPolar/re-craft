import java.lang.reflect.*;
import java.util.*;
import java.util.function.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.util.valueproviders.ConstantInt;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.FallenTreeFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public class FallenTreeOracle {
 static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
 static BlockState block(BlockPos p,int mode,Map<BlockPos,BlockState> blocks){
  var saved=blocks.get(p);if(saved!=null)return saved;
  int terrain=mode&3;boolean stone=p.getY()<10;
  if(terrain==1 && p.getY()==10 && Math.floorMod(p.getX()*3+p.getZ()*7,7)==0)stone=true;
  if(terrain==2 && p.getY()>=7 && Math.floorMod(p.getX()*3+p.getZ()*7,4)!=0)stone=false;
  if(terrain==3 && p.getY()>=5)stone=false;
  return (stone?Blocks.STONE:Blocks.AIR).defaultBlockState();
 }
 @SuppressWarnings("unchecked") public static void main(String[] args){
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  for(long seed:new long[]{0,1,-1,987654321})for(int length:new int[]{0,1,4,7,16})for(int mode=0;mode<8;mode++){
   final int scenario=mode;var blocks=new HashMap<BlockPos,BlockState>();var writes=new ArrayList<String>();var queries=new ArrayList<String>();
   var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
    if(m.getName().equals("isStateAtPosition")){var p=(BlockPos)v[0];queries.add("I"+pos(p));return ((Predicate<BlockState>)v[1]).test(block(p,scenario,blocks));}
    if(m.getName().equals("getBlockState")){var p=(BlockPos)v[0];queries.add("S"+pos(p));return block(p,scenario,blocks);}
    if(m.getName().equals("setBlockAndUpdate")){var p=((BlockPos)v[0]).immutable();var s=(BlockState)v[1];blocks.put(p,s);writes.add(pos(p)+","+s.getValue(BlockStateProperties.AXIS));return true;}
    throw new UnsupportedOperationException(m.toString());
   });
   var provider=(BlockStateProvider)Proxy.newProxyInstance(BlockStateProvider.class.getClassLoader(),new Class[]{BlockStateProvider.class},(p,m,v)->{
    if(m.getName().equals("getState")){if((scenario&4)!=0)((net.minecraft.util.RandomSource)v[1]).nextInt(17);return Blocks.OAK_LOG.defaultBlockState();}throw new UnsupportedOperationException(m.toString());
   });
   var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
   var feature=new FallenTreeFeature(Holder.direct(provider),ConstantInt.of(length),List.of(),List.of());
   boolean ok=feature.place(world,null,random,new BlockPos(-3,10,4));
   out.printf("C %d %d %d %s W %s Q %s R %d%n",seed,length,mode,ok,String.join(";",writes),String.join(";",queries),random.nextLong());
  }
 }
}
