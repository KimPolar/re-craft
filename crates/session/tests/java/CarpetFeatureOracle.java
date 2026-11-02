import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.blockpredicates.WouldSurvivePredicate;
import net.minecraft.world.level.levelgen.feature.SimpleBlockFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public class CarpetFeatureOracle {
  public static void main(String[] args) {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var names=List.of("moss_carpet","white_carpet","orange_carpet","magenta_carpet","light_blue_carpet","yellow_carpet","lime_carpet","pink_carpet","gray_carpet","light_gray_carpet","cyan_carpet","purple_carpet","blue_carpet","brown_carpet","green_carpet","red_carpet","black_carpet");
    var base=new BlockState[]{Blocks.AIR.defaultBlockState(),Blocks.CAVE_AIR.defaultBlockState(),Blocks.STONE.defaultBlockState(),Blocks.WATER.defaultBlockState(),Blocks.LAVA.defaultBlockState(),Blocks.SHORT_GRASS.defaultBlockState(),Blocks.MOSS_CARPET.defaultBlockState()};
    var current=new BlockState[]{Blocks.AIR.defaultBlockState(),Blocks.STONE.defaultBlockState(),Blocks.WATER.defaultBlockState(),Blocks.MOSS_CARPET.defaultBlockState()};
    var origin=new BlockPos(8,8,8);
    for(int kind=0;kind<names.size();kind++){
      var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+names.get(kind)));
      if(!(block instanceof CarpetBlock) || block.getStateDefinition().getPossibleStates().size()!=1)throw new AssertionError("not stateless carpet: "+block);
      var target=block.defaultBlockState();
      if(target.hasBlockEntity()||PoiTypes.hasPoi(target)||target.getPostProcessPos(null,BlockPos.ZERO)!=null)throw new AssertionError("unsafe carpet");
      for(int b=0;b<base.length;b++)for(int h=0;h<current.length;h++){
        var blocks=new HashMap<BlockPos,BlockState>();blocks.put(origin.below(),base[b]);blocks.put(origin,current[h]);
        var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
          var pos=(BlockPos)a[0];return switch(m.getName()){
            case "getBlockState" -> blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState());
            case "isEmptyBlock" -> blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState()).isAir();
            case "setBlock" -> {blocks.put(pos.immutable(),(BlockState)a[1]);yield true;}
            default -> throw new AssertionError(m.toString());
          };
        });
        boolean survives=target.canSurvive(world,origin);
        boolean predicate=new WouldSurvivePredicate(Vec3i.ZERO,target).test(world,origin);
        var random=new WorldgenRandom(new XoroshiroRandomSource(42));var feature=new SimpleBlockFeature(BlockStateProvider.of(target));
        boolean first=feature.place(world,null,random,origin),second=feature.place(world,null,random,origin);
        out.printf("C %d %d %d %s %s %s %s %s %d %s%n",kind,b,h,survives,predicate,first,second,BuiltInRegistries.BLOCK.getKey(blocks.get(origin).getBlock()),random.nextLong(),target.isSolid());
        var ticks=(ScheduledTickAccess)Proxy.newProxyInstance(ScheduledTickAccess.class.getClassLoader(),new Class[]{ScheduledTickAccess.class},(o,m,a)->{throw new AssertionError(m.toString());});
        for(var direction:Direction.values()){
          var edgeRandom=RandomSource.create(42);var updated=target.updateShape(world,ticks,origin,direction,origin.relative(direction),Blocks.STONE.defaultBlockState(),edgeRandom);
          if(edgeRandom.nextLong()!=RandomSource.create(42).nextLong())throw new AssertionError("unexpected random use");
          out.printf("U %d %d %d %s %s%n",kind,b,h,direction.getSerializedName(),BuiltInRegistries.BLOCK.getKey(updated.getBlock()));
        }
      }
    }
  }
}
