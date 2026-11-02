import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.random.WeightedList;
import net.minecraft.util.valueproviders.UniformInt;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.CaveVines;
import net.minecraft.world.level.block.GrowingPlantHeadBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.stateproviders.*;

public class RandomizedStateOracle {
  public static void main(String[] args) {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    for(String property:List.of("age","berries","missing"))for(long seed:new long[]{0,1,-1,42,987654321}) {
      var entries=WeightedList.<BlockState>builder().add(Blocks.CAVE_VINES.defaultBlockState(),2).add(Blocks.CAVE_VINES_PLANT.defaultBlockState(),1).add(Blocks.STONE.defaultBlockState(),1);
      var provider=new RandomizedIntStateProvider(Holder.direct(new WeightedStateProvider(entries)),property,UniformInt.of(23,26));
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));var samples=new ArrayList<String>();
      for(int i=0;i<12;i++){
        try {var s=provider.getState(null,random,BlockPos.ZERO);String extra=s.is(Blocks.CAVE_VINES)?",age="+s.getValue(GrowingPlantHeadBlock.AGE)+",berries="+s.getValue(CaveVines.BERRIES):s.is(Blocks.CAVE_VINES_PLANT)?",berries="+s.getValue(CaveVines.BERRIES):"";samples.add(BuiltInRegistries.BLOCK.getKey(s.getBlock())+extra);}
        catch(IllegalArgumentException ex){samples.add("ERR");}
      }
      out.printf("I %s %d %s R %d%n",property,seed,String.join(";",samples),random.nextLong());
    }
  }
}
