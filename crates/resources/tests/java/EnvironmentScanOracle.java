import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.placement.*;

public class EnvironmentScanOracle {
  static BlockState block(int y){return switch(Math.floorMod(y,5)){case 0,1->Blocks.AIR.defaultBlockState();case 2,3->Blocks.STONE.defaultBlockState();default->Blocks.DIRT.defaultBlockState();};}
  static BlockPredicate target(int kind){return switch(kind){case 0->BlockPredicate.matchesBlocks(Blocks.STONE);case 1->BlockPredicate.matchesBlocks(Blocks.AIR);case 2->BlockPredicate.insideWorld(new Vec3i(0,-2,0));default->BlockPredicate.allOf(BlockPredicate.not(BlockPredicate.matchesBlocks(Blocks.AIR)),BlockPredicate.insideWorld(new Vec3i(0,-2,0)));};}
  static BlockPredicate allowed(int kind){return switch(kind){case 0->BlockPredicate.alwaysTrue();case 1->BlockPredicate.matchesBlocks(Blocks.AIR);case 2->BlockPredicate.matchesBlocks(Blocks.STONE);default->BlockPredicate.not(BlockPredicate.matchesBlocks(Blocks.DIRT));};}
  public static void main(String[]args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var level=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->switch(m.getName()){
      case "getBlockState"->block(((BlockPos)a[0]).getY());
      case "isStateAtPosition"->((Predicate<BlockState>)a[1]).test(block(((BlockPos)a[0]).getY()));
      case "isInsideBuildHeight"->{int y=((BlockPos)a[0]).getY();yield y>=-4&&y<5;}
      case "isOutsideBuildHeight"->{int y=(Integer)a[0];yield y< -4||y>=5;}
      default->throw new UnsupportedOperationException(m.getName());
    });
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);var unsafe=(sun.misc.Unsafe)field.get(null);
    var context=(PlacementContext)unsafe.allocateInstance(PlacementContext.class);
    var levelField=PlacementContext.class.getDeclaredField("level");levelField.setAccessible(true);levelField.set(context,level);
    for(var name:List.of("minY","height")){var f=WorldGenerationContext.class.getDeclaredField(name);f.setAccessible(true);f.setInt(context,name.equals("minY")?-64:384);}
    int n=0;
    for(var direction:List.of(Direction.UP,Direction.DOWN))for(int steps:new int[]{1,2,5,32})for(int y=-6;y<=7;y++)for(int target=0;target<4;target++)for(int allow=0;allow<4;allow++)for(long seed:new long[]{0,7}){
      var scan=new EnvironmentScanPlacement(direction,target(target),allowed(allow),steps);var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      var result=new ArrayList<BlockPos>();scan.modify(context,random,new BlockPos(0,y,0),p->result.add(p.immutable()));
      out.printf("C %d %s %d %d %d %d %d %d %d %d%n",n++,direction.getName(),steps,y,target,allow,seed,result.size(),result.isEmpty()?0:result.getFirst().getY(),random.nextLong());
    }
  }
}
