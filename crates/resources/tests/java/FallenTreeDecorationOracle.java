import java.lang.reflect.*;
import java.util.*;
import java.util.function.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.treedecorators.*;

public class FallenTreeDecorationOracle {
 static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
 @SuppressWarnings("unchecked") public static void main(String[] args){
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  for(long seed:new long[]{0,1,-1,987654321})for(int mode=0;mode<3;mode++)for(int choice=0;choice<3;choice++)for(float probability:new float[]{0f,0.1f,0.5f,1f})for(boolean vines:new boolean[]{false,true}){
   final int scenario=mode;var logs=new HashSet<BlockPos>();logs.add(new BlockPos(-3,10,4));logs.add(new BlockPos(-2,10,4));logs.add(new BlockPos(-1,10,4));logs.add(new BlockPos(-1,11,4));
   var blocks=new HashMap<BlockPos,BlockState>();for(var p:logs)blocks.put(p,Blocks.OAK_LOG.defaultBlockState());var writes=new ArrayList<String>();var queries=new ArrayList<String>();
   var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
    if(m.getName().equals("isStateAtPosition")){var p=(BlockPos)v[0];queries.add(pos(p));var s=blocks.get(p);if(s==null)s=(((scenario==1 && Math.floorMod(p.getX()*3+p.getY()*5+p.getZ()*7,5)==0)||scenario==2)?Blocks.STONE:Blocks.AIR).defaultBlockState();return ((Predicate<BlockState>)v[1]).test(s);}
    throw new UnsupportedOperationException(m.toString());
   });
   var provider=(BlockStateProvider)Proxy.newProxyInstance(BlockStateProvider.class.getClassLoader(),new Class[]{BlockStateProvider.class},(p,m,v)->{
    if(m.getName().equals("getState"))return ((net.minecraft.util.RandomSource)v[1]).nextInt(3)<2?Blocks.RED_MUSHROOM.defaultBlockState():Blocks.BROWN_MUSHROOM.defaultBlockState();throw new UnsupportedOperationException(m.toString());
   });
   var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
   BiConsumer<BlockPos,BlockState> setter=(p,s)->{p=p.immutable();blocks.put(p,s);String kind;if(s.is(Blocks.VINE)){kind="vine:"+(s.getValue(BlockStateProperties.EAST)?"east":s.getValue(BlockStateProperties.WEST)?"west":s.getValue(BlockStateProperties.NORTH)?"north":"south");}else kind=s.is(Blocks.RED_MUSHROOM)?"red":"brown";writes.add(pos(p)+","+kind);};
   var context=new TreeDecorator.Context(world,setter,random,logs,Set.of(),Set.of());
   if(vines)TrunkVineDecorator.INSTANCE.place(context);
   var directions=choice==0?List.of(Direction.UP):choice==1?List.of(Direction.NORTH,Direction.UP):List.of(Direction.WEST,Direction.SOUTH,Direction.UP);
   new AttachedToLogsDecorator(probability,Holder.direct(provider),directions).place(context);
   out.printf("C %d %d %d %s %s W %s Q %s R %d%n",seed,mode,choice,Float.toString(probability),vines,String.join(";",writes),String.join(";",queries),random.nextLong());
  }
 }
}
