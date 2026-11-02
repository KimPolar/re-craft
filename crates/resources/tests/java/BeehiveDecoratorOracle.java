import java.lang.reflect.*;
import java.util.*;
import java.util.function.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.BeehiveBlock;
import net.minecraft.world.level.block.entity.BeehiveBlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.treedecorators.BeehiveDecorator;
import net.minecraft.world.level.levelgen.feature.treedecorators.TreeDecorator;

public class BeehiveDecoratorOracle {
  static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
  @SuppressWarnings("unchecked") public static void main(String[] args)throws Exception{
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var getBees=BeehiveBlockEntity.class.getDeclaredMethod("getBees");getBees.setAccessible(true);
    for(long seed:new long[]{0,1,-1,987654321})for(int mode=0;mode<6;mode++)for(float probability:new float[]{0f,0.05f,0.5f,1f}){
      final int scenario=mode;
      var logs=new HashSet<BlockPos>();var leaves=new HashSet<BlockPos>();
      if(mode!=0){logs.add(new BlockPos(5,4,5));logs.add(new BlockPos(5,5,5));logs.add(new BlockPos(5,6,5));logs.add(new BlockPos(5,7,5));}
      if(mode==5){logs.add(new BlockPos(6,6,5));logs.add(new BlockPos(4,6,5));}
      if(mode!=2 && mode!=0){leaves.add(new BlockPos(5,7,4));leaves.add(new BlockPos(6,7,5));leaves.add(new BlockPos(4,7,5));}
      var solid=new HashSet<BlockPos>();solid.addAll(logs);solid.addAll(leaves);
      if(mode==3)for(int x=1;x<=9;x++)for(int y=4;y<=9;y++)for(int z=1;z<=9;z++)solid.add(new BlockPos(x,y,z));
      var queries=new ArrayList<String>();var nest=new BlockPos[1];var entity=new BeehiveBlockEntity[1];
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
        if(m.getName().equals("isStateAtPosition")){var p=(BlockPos)v[0];queries.add(pos(p));return ((Predicate<BlockState>)v[1]).test((solid.contains(p)?Blocks.STONE:Blocks.AIR).defaultBlockState());}
        if(m.getName().equals("getBlockEntity")){if(scenario==4)return Optional.empty();entity[0]=new BeehiveBlockEntity((BlockPos)v[0],Blocks.BEE_NEST.defaultBlockState());return Optional.of(entity[0]);}
        throw new UnsupportedOperationException(m.toString());
      });
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      BiConsumer<BlockPos,BlockState> setter=(p,s)->{nest[0]=p.immutable();if(!s.is(Blocks.BEE_NEST)||s.getValue(BeehiveBlock.FACING)!=Direction.SOUTH)throw new AssertionError("unexpected bee nest state");};
      var context=new TreeDecorator.Context(world,setter,random,logs,leaves,Set.of());
      new BeehiveDecorator(probability).place(context);
      var times=new ArrayList<String>();
      if(entity[0]!=null)for(var bee:(List<BeehiveBlockEntity.Occupant>)getBees.invoke(entity[0]))times.add(Integer.toString(bee.ticksInHive()));
      out.printf("C %d %d %s N %s B %s Q %s R %d%n",seed,mode,Float.toString(probability),nest[0]==null?"-":pos(nest[0]),String.join(",",times),String.join(";",queries),random.nextLong());
    }
  }
}
