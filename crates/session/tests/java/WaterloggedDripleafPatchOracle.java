import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.valueproviders.ConstantInt;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BooleanProperty;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.*;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.placement.*;

public class WaterloggedDripleafPatchOracle {
  static String key(BlockState s){var p=new TreeMap<String,String>();s.getValues().forEach(v->p.put(v.property().getName(),v.valueName()));return BuiltInRegistries.BLOCK.getKey(s.getBlock())+"|"+p.entrySet().stream().map(e->e.getKey()+"="+e.getValue()+";").reduce("",String::concat);}
  public static void main(String[]args){
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    for(var block:List.of(Blocks.BIG_DRIPLEAF,Blocks.BIG_DRIPLEAF_STEM,Blocks.SMALL_DRIPLEAF)){
      var water=(BooleanProperty)block.getStateDefinition().getProperty("waterlogged");
      for(var state:block.getStateDefinition().getPossibleStates()){
        var changed=state.getValue(water) ? state : state.setValue(water,true);
        out.printf("S %s %s %s%n",key(state),key(changed),state!=changed);
      }
    }
    for(int kind=0;kind<3;kind++)for(boolean wet:new boolean[]{false,true})for(long seed:new long[]{0,42,-1}){
      var block=kind==0?Blocks.BIG_DRIPLEAF:kind==1?Blocks.BIG_DRIPLEAF_STEM:Blocks.SMALL_DRIPLEAF;
      var water=(BooleanProperty)block.getStateDefinition().getProperty("waterlogged");
      var nestedState=block.defaultBlockState().setValue(water,wet);
      var column=new BlockColumnFeature(List.of(BlockColumnFeature.layer(ConstantInt.of(1),BlockStateProvider.of(nestedState))),Direction.UP,BlockPredicate.matchesBlocks(Blocks.AIR),true);
      var nested=new PlacedFeature(Holder.direct(column),List.of());
      var feature=new WaterloggedVegetationPatchFeature(HolderSet.direct(Blocks.STONE.builtInRegistryHolder()),Holder.direct(BlockStateProvider.of(Blocks.CLAY)),Holder.direct(nested),CaveSurface.FLOOR,ConstantInt.of(1),0.0f,3,1.0f,ConstantInt.of(0),0.0f);
      var changes=new LinkedHashMap<BlockPos,BlockState>();
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
        if(m.getName().equals("getMinY"))return 0;if(m.getName().equals("getHeight"))return 16;
        var p=(BlockPos)a[0];var current=changes.getOrDefault(p,p.getY()<8?Blocks.STONE.defaultBlockState():Blocks.AIR.defaultBlockState());
        return switch(m.getName()){
          case "getBlockState"->current;
          case "isEmptyBlock"->current.isAir();
          case "isStateAtPosition"->((Predicate<BlockState>)a[1]).test(current);
          case "setBlock"->{changes.put(p.immutable(),(BlockState)a[1]);yield true;}
          default->throw new UnsupportedOperationException(m.toString());
        };
      });
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      boolean result=feature.place(world,new VegetationPatchOracle.Generator(),random,new BlockPos(8,8,8));
      out.printf("P %d %s %d %s %d%n",kind,wet,seed,result,random.nextLong());
      for(var entry:changes.entrySet()){var p=entry.getKey();out.printf("B %d %d %d %s%n",p.getX(),p.getY(),p.getZ(),key(entry.getValue()));}
    }
  }
}
