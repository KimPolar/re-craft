import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeReader;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.*;

public class TreeEdgeFluidOracle {
  public static void main(String[]args)throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.MAGMA_BLOCK.builtInRegistryHolder(),List.of(BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN));
    bind.invoke(Blocks.SOUL_SAND.builtInRegistryHolder(),List.of(BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP));
    bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP));
    var env=(EnvironmentAttributeReader)Proxy.newProxyInstance(EnvironmentAttributeReader.class.getClassLoader(),new Class[]{EnvironmentAttributeReader.class},(o,m,a)->{if(m.getName().equals("getDimensionValue"))return false;throw new UnsupportedOperationException(m.toString());});
    var level=(LevelReader)Proxy.newProxyInstance(LevelReader.class.getClassLoader(),new Class[]{LevelReader.class},(o,m,a)->{if(m.getName().equals("environmentAttributes"))return env;throw new UnsupportedOperationException(m.toString());});
    var neighbors=new BlockState[]{Blocks.AIR.defaultBlockState(),Blocks.WATER.defaultBlockState(),Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,1),Blocks.LAVA.defaultBlockState(),Blocks.MAGMA_BLOCK.defaultBlockState(),Blocks.SOUL_SAND.defaultBlockState(),Blocks.STONE.defaultBlockState()};
    var directions=new Direction[]{Direction.DOWN,Direction.UP,Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
    for(int mode=0;mode<4;mode++){
      bind.invoke(Fluids.WATER.builtInRegistryHolder(),(mode&1)!=0?List.of(FluidTags.BUBBLE_COLUMN_CAN_OCCUPY):List.of());
      bind.invoke(Fluids.LAVA.builtInRegistryHolder(),(mode&2)!=0?List.of(FluidTags.BUBBLE_COLUMN_CAN_OCCUPY):List.of());
      for(int kind=0;kind<2;kind++)for(int depth:new int[]{0,1,8,15})for(int n=0;n<neighbors.length;n++)for(Direction direction:directions){
        var state=(kind==0?Blocks.WATER:Blocks.LAVA).defaultBlockState().setValue(LiquidBlock.LEVEL,depth);
        var events=new ArrayList<String>();
        var ticks=(ScheduledTickAccess)Proxy.newProxyInstance(ScheduledTickAccess.class.getClassLoader(),new Class[]{ScheduledTickAccess.class},(o,m,a)->{
          if(m.getName().equals("scheduleTick")){Object type=a[1];events.add(type instanceof Fluid?"F:"+BuiltInRegistries.FLUID.getKey((Fluid)type):"B:"+BuiltInRegistries.BLOCK.getKey((Block)type));return null;}
          throw new UnsupportedOperationException(m.toString());
        });
        var pos=new BlockPos(8,8,8);var updated=state.updateShape(level,ticks,pos,direction,pos.relative(direction),neighbors[n],RandomSource.create(42));
        out.printf("E %d %d %d %d %s %s %s%n",mode,kind,depth,n,direction.getSerializedName(),updated==state,String.join(",",events));
      }
    }
  }
}
