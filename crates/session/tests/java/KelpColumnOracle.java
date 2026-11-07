import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.valueproviders.*;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.BlockColumnFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.*;

public class KelpColumnOracle {
  public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var head=new RandomizedIntStateProvider(BlockStateProvider.of(Blocks.KELP),GrowingPlantHeadBlock.AGE,UniformInt.of(20,23));
    var layers=List.of(BlockColumnFeature.layer(UniformInt.of(0,9),BlockStateProvider.of(Blocks.KELP_PLANT)),BlockColumnFeature.layer(ConstantInt.of(1),head));
    var allowed=BlockPredicate.allOf(BlockPredicate.matchesBlocks(Blocks.WATER),BlockPredicate.matchesBlocks(Direction.UP,Blocks.WATER));
    var feature=new BlockColumnFeature(layers,Direction.UP,allowed,true);
    var origin=new BlockPos(8,2,8);
    for(int depth=1;depth<=12;depth++) for(int level:new int[]{0,3}) for(boolean restricted:new boolean[]{false,true}) for(long seed:new long[]{0,1,7,-1,123456}) {
      final int waterDepth=depth;
      var writes=new LinkedHashMap<BlockPos,BlockState>();
      var water=Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,level);
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(obj,method,a)->{
        var p=(BlockPos)a[0];
        var initial=p.getX()==8 && p.getZ()==8 && p.getY()>=2 && p.getY()<2+waterDepth ? water : Blocks.AIR.defaultBlockState();
        var current=writes.getOrDefault(p,initial);
        return switch(method.getName()) {
          case "getBlockState" -> current;
          case "isStateAtPosition" -> ((Predicate<BlockState>)a[1]).test(current);
          case "setBlock" -> { if(!restricted) writes.put(p.immutable(),(BlockState)a[1]); yield !restricted; }
          default -> throw new UnsupportedOperationException(method.toString());
        };
      });
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      boolean result=feature.place(world,null,random,origin);
      var changes=new ArrayList<String>();
      for(var entry:writes.entrySet()) {
        var s=entry.getValue();
        var age=s.is(Blocks.KELP)?s.getValue(GrowingPlantHeadBlock.AGE).toString():"-";
        changes.add(entry.getKey().getY()+":"+BuiltInRegistries.BLOCK.getKey(s.getBlock())+":"+age);
      }
      out.printf("K %d %d %s %d %s %d %s%n",depth,level,restricted,seed,result,random.nextLong(),changes.isEmpty()?"-":String.join(",",changes));
    }
  }
}
