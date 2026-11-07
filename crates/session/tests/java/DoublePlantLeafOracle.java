import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.SimpleBlockFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public class DoublePlantLeafOracle {
  public static void main(String[] args)throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.SUPPORTS_VEGETATION));
    var origin=new BlockPos(8,8,8);
    for(var block:List.of(Blocks.OAK_LEAVES,Blocks.DARK_OAK_LEAVES,Blocks.BIRCH_LEAVES,Blocks.AZALEA_LEAVES,Blocks.FLOWERING_AZALEA_LEAVES))
      for(var leaf:block.getStateDefinition().getPossibleStates()) {
        if(leaf.canBeReplaced())throw new AssertionError("replaceable leaf: "+leaf);
        var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
          var p=(BlockPos)a[0];
          return switch(m.getName()) {
            case "getBlockState" -> p.equals(origin.below())?Blocks.STONE.defaultBlockState():p.equals(origin.above())?leaf:Blocks.AIR.defaultBlockState();
            case "setBlock" -> throw new AssertionError("unexpected double-plant write");
            default -> throw new AssertionError("unexpected query "+m);
          };
        });
        var random=new WorldgenRandom(new XoroshiroRandomSource(42));
        var feature=new SimpleBlockFeature(BlockStateProvider.of(Blocks.TALL_GRASS));
        boolean result=feature.place(world,null,random,origin);
        out.printf("L %s %s %s %s %s %d%n",BuiltInRegistries.BLOCK.getKey(block),leaf.getValue(LeavesBlock.DISTANCE),leaf.getValue(LeavesBlock.PERSISTENT),leaf.getValue(LeavesBlock.WATERLOGGED),result,random.nextLong());
      }
  }
}
