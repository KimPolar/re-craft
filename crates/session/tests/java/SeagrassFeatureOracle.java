import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.*;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.DoubleBlockHalf;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.SimpleBlockFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public class SeagrassFeatureOracle {
  static String key(BlockState s){var p=new TreeMap<String,String>();s.getValues().forEach(v->p.put(v.property().getName(),v.valueName()));return BuiltInRegistries.BLOCK.getKey(s.getBlock())+"|"+p.entrySet().stream().map(e->e.getKey()+"="+e.getValue()+";").reduce("",String::concat);}
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    for(var block:List.of(Blocks.SEAGRASS,Blocks.TALL_SEAGRASS))for(var state:block.getStateDefinition().getPossibleStates())
      if(state.hasBlockEntity()||PoiTypes.hasPoi(state)||state.getPostProcessPos(null,BlockPos.ZERO)!=null)throw new AssertionError("unsupported seagrass effects "+state);
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.MAGMA_BLOCK.builtInRegistryHolder(),List.of(BlockTags.CANNOT_SUPPORT_SEAGRASS));
    var base=new BlockState[]{Blocks.STONE.defaultBlockState(),Blocks.SAND.defaultBlockState(),Blocks.MAGMA_BLOCK.defaultBlockState(),Blocks.AIR.defaultBlockState(),Blocks.TALL_SEAGRASS.defaultBlockState()};
    var positions=new BlockState[]{Blocks.AIR.defaultBlockState(),Blocks.WATER.defaultBlockState(),Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,1),Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,8),Blocks.LAVA.defaultBlockState(),Blocks.SEAGRASS.defaultBlockState(),Blocks.TALL_SEAGRASS.defaultBlockState(),Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF,DoubleBlockHalf.UPPER)};
    var origin=new BlockPos(8,8,8);
    for(int mode=0;mode<8;mode++){
      bind.invoke(Fluids.WATER.builtInRegistryHolder(),(mode&1)!=0?List.of(FluidTags.WATER):List.of());
      bind.invoke(Fluids.FLOWING_WATER.builtInRegistryHolder(),(mode&2)!=0?List.of(FluidTags.WATER):List.of());
      bind.invoke(Blocks.STONE.builtInRegistryHolder(),(mode&4)!=0?List.of(BlockTags.CANNOT_SUPPORT_SEAGRASS):List.of());
      for(int kind=0;kind<3;kind++)for(int b=0;b<base.length;b++)for(int here=0;here<positions.length;here++)for(int above=0;above<positions.length;above++){
        var blocks=new HashMap<BlockPos,BlockState>();blocks.put(origin.below(),base[b]);blocks.put(origin,positions[here]);blocks.put(origin.above(),positions[above]);
        var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
          var p=(BlockPos)a[0];return switch(m.getName()){
            case "getBlockState" -> blocks.getOrDefault(p,Blocks.AIR.defaultBlockState());
            case "getFluidState" -> blocks.getOrDefault(p,Blocks.AIR.defaultBlockState()).getFluidState();
            case "setBlock" -> {blocks.put(p.immutable(),(BlockState)a[1]);yield true;}
            default -> throw new UnsupportedOperationException(m.toString());
          };
        });
        var state=kind==0?Blocks.SEAGRASS.defaultBlockState():Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF,kind==1?DoubleBlockHalf.LOWER:DoubleBlockHalf.UPPER);
        var feature=new SimpleBlockFeature(BlockStateProvider.of(state));var random=new WorldgenRandom(new XoroshiroRandomSource(42));
        boolean result=feature.place(world,null,random,origin);boolean second=feature.place(world,null,random,origin);
        out.printf("S %d %d %d %d %d %s %s %s %s %d%n",mode,kind,b,here,above,result,second,key(blocks.get(origin)),key(blocks.get(origin.above())),random.nextLong());
      }
    }
  }
}
