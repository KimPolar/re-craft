import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.SimpleBlockFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public class SporeBlossomOracle {
  public static void main(String[] args) throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
    var target=Blocks.SPORE_BLOSSOM.defaultBlockState();
    if(target.hasBlockEntity() || PoiTypes.hasPoi(target) || target.getPostProcessPos(null,BlockPos.ZERO)!=null) throw new AssertionError("unsafe spore blossom");
    if(target.isSolid()) throw new AssertionError("spore blossom unexpectedly solid");
    var supports=new Block[]{Blocks.AIR,Blocks.STONE,Blocks.DIRT,Blocks.WATER};
    var current=new BlockState[]{Blocks.AIR.defaultBlockState(),Blocks.WATER.defaultBlockState(),Blocks.LAVA.defaultBlockState()};
    var origin=new BlockPos(8,8,8);
    for(int mode=0;mode<4;mode++) {
      var tags=new ArrayList<net.minecraft.tags.TagKey<Block>>();
      if((mode&1)!=0) tags.add(BlockTags.UNSTABLE_BOTTOM_CENTER);
      bind.invoke(Blocks.STONE.builtInRegistryHolder(),tags);
      final boolean waterTagged=(mode&2)!=0;
      for(int s=0;s<supports.length;s++) for(int h=0;h<current.length;h++) {
        var blocks=new HashMap<BlockPos,BlockState>(); blocks.put(origin.above(),supports[s].defaultBlockState()); blocks.put(origin,current[h]);
        var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
          var pos=(BlockPos)a[0]; return switch(m.getName()) {
            case "getBlockState" -> blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState());
            case "isWaterAt" -> waterTagged && blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState()).is(Blocks.WATER);
            case "setBlock" -> { blocks.put(pos.immutable(),(BlockState)a[1]); yield true; }
            default -> throw new AssertionError(m.toString());
          };
        });
        boolean survives=target.canSurvive(world,origin);
        var random=new WorldgenRandom(new XoroshiroRandomSource(42));
        var feature=new SimpleBlockFeature(BlockStateProvider.of(target));
        boolean first=feature.place(world,null,random,origin), second=feature.place(world,null,random,origin);
        out.printf("S %d %d %d %s %s %s %s %d%n",mode,s,h,survives,first,second,BuiltInRegistries.BLOCK.getKey(blocks.get(origin).getBlock()),random.nextLong());
      }
    }
  }
}
