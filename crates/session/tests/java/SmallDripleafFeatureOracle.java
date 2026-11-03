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
import net.minecraft.world.level.block.state.properties.BooleanProperty;
import net.minecraft.world.level.block.state.properties.DoubleBlockHalf;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.SimpleBlockFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public class SmallDripleafFeatureOracle {
  static String key(BlockState s){var p=new TreeMap<String,String>();s.getValues().forEach(v->p.put(v.property().getName(),v.valueName()));return BuiltInRegistries.BLOCK.getKey(s.getBlock())+"|"+p.entrySet().stream().map(e->e.getKey()+"="+e.getValue()+";").reduce("",String::concat);}
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    for(var block:List.of(Blocks.CAVE_VINES,Blocks.CAVE_VINES_PLANT))for(var state:block.getStateDefinition().getPossibleStates())
      if(state.canBeReplaced()||!state.getFluidState().isEmpty())throw new AssertionError("unexpected cave vine replacement state "+state);
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    var wet=(BooleanProperty)Blocks.SMALL_DRIPLEAF.getStateDefinition().getProperty("waterlogged");
    var lower=Blocks.SMALL_DRIPLEAF.defaultBlockState().setValue(DoublePlantBlock.HALF,DoubleBlockHalf.LOWER);
    var upper=lower.setValue(DoublePlantBlock.HALF,DoubleBlockHalf.UPPER).setValue(wet,true);
    var base=new BlockState[]{Blocks.STONE.defaultBlockState(),Blocks.AIR.defaultBlockState(),lower,upper};
    var positions=new BlockState[]{Blocks.AIR.defaultBlockState(),Blocks.WATER.defaultBlockState(),Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,1),Blocks.LAVA.defaultBlockState(),lower,upper,Blocks.SEAGRASS.defaultBlockState(),Blocks.STONE.defaultBlockState()};
    var abovePositions=Arrays.copyOf(positions,12);
    abovePositions[8]=Blocks.CAVE_VINES.defaultBlockState();abovePositions[9]=abovePositions[8].setValue(CaveVines.BERRIES,true);
    abovePositions[10]=Blocks.CAVE_VINES_PLANT.defaultBlockState();abovePositions[11]=abovePositions[10].setValue(CaveVines.BERRIES,true);
    var origin=new BlockPos(8,8,8);
    for(int mode=0;mode<32;mode++){
      var tags=new ArrayList<TagKey<net.minecraft.world.level.block.Block>>();
      if((mode&1)!=0)tags.add(BlockTags.SUPPORTS_SMALL_DRIPLEAF);
      if((mode&2)!=0)tags.add(BlockTags.SUPPORTS_VEGETATION);
      bind.invoke(Blocks.STONE.builtInRegistryHolder(),tags);
      bind.invoke(Fluids.WATER.builtInRegistryHolder(),(mode&4)!=0?List.of(FluidTags.WATER):List.of());
      bind.invoke(Fluids.FLOWING_WATER.builtInRegistryHolder(),(mode&8)!=0?List.of(FluidTags.WATER):List.of());
      bind.invoke(Fluids.LAVA.builtInRegistryHolder(),(mode&16)!=0?List.of(FluidTags.WATER):List.of());
      for(int half=0;half<2;half++)for(int targetWet=0;targetWet<2;targetWet++)for(int b=0;b<base.length;b++)for(int here=0;here<positions.length;here++)for(int above=0;above<abovePositions.length;above++){
        var blocks=new HashMap<BlockPos,BlockState>();blocks.put(origin.below(),base[b]);blocks.put(origin,positions[here]);blocks.put(origin.above(),abovePositions[above]);
        var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
          var p=(BlockPos)a[0];var state=blocks.getOrDefault(p,Blocks.AIR.defaultBlockState());return switch(m.getName()){
            case "getBlockState" -> state;
            case "getFluidState" -> state.getFluidState();
            case "isWaterAt" -> state.getFluidState().is(FluidTags.WATER);
            case "setBlock" -> {blocks.put(p.immutable(),(BlockState)a[1]);yield true;}
            default -> throw new UnsupportedOperationException(m.toString());
          };
        });
        var target=lower.setValue(DoublePlantBlock.HALF,half==0?DoubleBlockHalf.LOWER:DoubleBlockHalf.UPPER).setValue(wet,targetWet!=0);
        boolean survives=target.canSurvive(world,origin);
        var feature=new SimpleBlockFeature(BlockStateProvider.of(target));var random=new WorldgenRandom(new XoroshiroRandomSource(42));
        boolean result=feature.place(world,null,random,origin);boolean second=feature.place(world,null,random,origin);
        out.printf("S %d %d %d %d %d %d %s %s %s %s %s %d%n",mode,half,targetWet,b,here,above,survives,result,second,key(blocks.get(origin)),key(blocks.get(origin.above())),random.nextLong());
      }
    }
  }
}
