import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.feature.SpringFeature;
import net.minecraft.world.level.material.*;
import net.minecraft.world.ticks.*;

public class SpringFeatureOracle {
  static BlockState block(int n) { return switch(n) {case 0 -> Blocks.AIR.defaultBlockState(); case 1 -> Blocks.STONE.defaultBlockState(); default -> Blocks.DIRT.defaultBlockState();}; }
  public static void main(String[] args) throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    if(Blocks.WATER.defaultBlockState().getPostProcessPos(null,BlockPos.ZERO)!=null || Blocks.LAVA.defaultBlockState().getPostProcessPos(null,BlockPos.ZERO)!=null) throw new AssertionError("fluid postprocessing");
    for(var fluid:List.of(Fluids.FLOWING_WATER,Fluids.FLOWING_LAVA)) {
      var id=BuiltInRegistries.FLUID.getKey(fluid);
      var def=fluid.defaultFluidState();
      out.println("F "+id+" default default "+def.createLegacyBlock().getValue(net.minecraft.world.level.block.LiquidBlock.LEVEL));
      for(int level=1;level<=8;level++)for(boolean falling:new boolean[]{false,true}) {
        var state=fluid.getFlowing(level,falling);
        out.println("F "+id+" "+level+" "+falling+" "+state.createLegacyBlock().getValue(net.minecraft.world.level.block.LiquidBlock.LEVEL));
      }
    }
    var p=BlockPos.ZERO;
    var positions=List.of(p.above(),p.below(),p,p.west(),p.east(),p.north(),p.south());
    for(int code=0;code<2187;code++) for(int config=0;config<16;config++) {
      var map=new HashMap<BlockPos,BlockState>();int v=code;for(var pos:positions){map.put(pos,block(v%3));v/=3;}
      var ticks=new ProtoChunkTicks<Fluid>();boolean restricted=(config&4)!=0;
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
        var pos=(BlockPos)a[0];
        return switch(m.getName()) {
          case "getBlockState" -> map.getOrDefault(pos,Blocks.AIR.defaultBlockState());
          case "isEmptyBlock" -> map.getOrDefault(pos,Blocks.AIR.defaultBlockState()).isAir();
          case "setBlock" -> {if(!restricted)map.put(pos,(BlockState)a[1]);yield !restricted;}
          case "scheduleTick" -> {ticks.schedule(new ScheduledTick<>((Fluid)a[1],pos,1L,0L));yield null;}
          default -> throw new UnsupportedOperationException(m.getName());
        };
      });
      var fluid=(config&1)==0 ? Fluids.WATER : Fluids.LAVA;
      boolean below=(config&2)==0;
      var feature=new SpringFeature((config&8)==0 ? fluid.getSource(true) : fluid.getFlowing(3,true),below,4,1,HolderSet.direct(Blocks.STONE.builtInRegistryHolder()));
      boolean first=feature.place(world,null,null,p);boolean second=feature.place(world,null,null,p);
      var state=map.get(p);String name=BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString();
      String level=state.hasProperty(net.minecraft.world.level.block.LiquidBlock.LEVEL)?state.getValue(net.minecraft.world.level.block.LiquidBlock.LEVEL).toString():"-";
      String tick=ticks.scheduledTicks().isEmpty()?"-":BuiltInRegistries.FLUID.getKey(ticks.scheduledTicks().getFirst().type()).toString();
      out.printf("S %d %d %s %s %s %s %s %d%n",code,config,first,second,name,level,tick,ticks.scheduledTicks().size());
    }
  }
}
