import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.blockpredicates.WouldSurvivePredicate;
import net.minecraft.world.level.levelgen.feature.SimpleBlockFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public class AzaleaFeatureOracle {
  public static void main(String[]args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    var plants=new Block[]{Blocks.AZALEA,Blocks.FLOWERING_AZALEA};
    for(var plant:plants) for(var state:plant.getStateDefinition().getPossibleStates())
      if(state.hasBlockEntity()||PoiTypes.hasPoi(state)||state.getPostProcessPos(null,BlockPos.ZERO)!=null)throw new AssertionError("unsafe azalea state");
    var origin=new BlockPos(8,8,8);
    var bases=new Block[]{Blocks.STONE,Blocks.DIRT,Blocks.SAND,Blocks.AIR};
    var current=new BlockState[]{Blocks.AIR.defaultBlockState(),Blocks.STONE.defaultBlockState(),Blocks.WATER.defaultBlockState(),Blocks.AZALEA.defaultBlockState(),Blocks.FLOWERING_AZALEA.defaultBlockState()};
    for(int mode=0;mode<8;mode++){
      for(int i=0;i<bases.length;i++){
        var tags=new ArrayList<net.minecraft.tags.TagKey<Block>>();
        if((i==0 && (mode&1)!=0)||(i==1 && (mode&2)!=0))tags.add(BlockTags.SUPPORTS_AZALEA);
        if(i==0 && (mode&4)!=0)tags.add(BlockTags.SUPPORTS_VEGETATION);
        bind.invoke(bases[i].builtInRegistryHolder(),tags);
      }
      for(int kind=0;kind<2;kind++)for(int base=0;base<bases.length;base++)for(int here=0;here<current.length;here++){
        var blocks=new HashMap<BlockPos,BlockState>();blocks.put(origin.below(),bases[base].defaultBlockState());blocks.put(origin,current[here]);
        var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
          var pos=(BlockPos)a[0];return switch(m.getName()){
            case "getBlockState" -> blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState());
            case "setBlock" -> {blocks.put(pos.immutable(),(BlockState)a[1]);yield true;}
            default -> throw new AssertionError(m.toString());
          };
        });
        var target=plants[kind].defaultBlockState();
        boolean survives=target.canSurvive(world,origin);
        boolean predicate=new WouldSurvivePredicate(Vec3i.ZERO,target).test(world,origin);
        var random=new WorldgenRandom(new XoroshiroRandomSource(42));var feature=new SimpleBlockFeature(BlockStateProvider.of(target));
        boolean first=feature.place(world,null,random,origin),second=feature.place(world,null,random,origin);
        out.printf("A %d %d %d %d %s %s %s %s %s %d %s%n",mode,kind,base,here,survives,predicate,first,second,BuiltInRegistries.BLOCK.getKey(blocks.get(origin).getBlock()),random.nextLong(),target.isSolid());
        var ticks=(ScheduledTickAccess)Proxy.newProxyInstance(ScheduledTickAccess.class.getClassLoader(),new Class[]{ScheduledTickAccess.class},(o,m,a)->{throw new AssertionError(m.toString());});
        for(var direction:Direction.values()){
          var edgeRandom=RandomSource.create(42);var updated=target.updateShape(world,ticks,origin,direction,origin.relative(direction),Blocks.STONE.defaultBlockState(),edgeRandom);
          if(edgeRandom.nextLong()!=RandomSource.create(42).nextLong())throw new AssertionError("unexpected RNG use");
          out.printf("U %d %d %d %d %s %s%n",mode,kind,base,here,direction.getSerializedName(),BuiltInRegistries.BLOCK.getKey(updated.getBlock()));
        }
      }
    }
  }
}
