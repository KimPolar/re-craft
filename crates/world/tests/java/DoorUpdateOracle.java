import java.lang.reflect.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;

public class DoorUpdateOracle {
 static String describe(BlockState s){
  var name=BuiltInRegistries.BLOCK.getKey(s.getBlock()).toString();
  if(!(s.getBlock() instanceof DoorBlock))return name+" - - - - -";
  return name+" "+s.getValue(DoorBlock.FACING).getName()+" "+s.getValue(DoorBlock.HALF).getSerializedName()+" "+s.getValue(DoorBlock.HINGE).getSerializedName()+" "+s.getValue(DoorBlock.OPEN)+" "+s.getValue(DoorBlock.POWERED);
 }
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var method=DoorBlock.class.getDeclaredMethod("updateShape",BlockState.class,LevelReader.class,ScheduledTickAccess.class,BlockPos.class,Direction.class,BlockPos.class,BlockState.class,RandomSource.class);method.setAccessible(true);
  var scheduled=(ScheduledTickAccess)Proxy.newProxyInstance(ScheduledTickAccess.class.getClassLoader(),new Class<?>[]{ScheduledTickAccess.class},(p,m,v)->{throw new AssertionError("unexpected "+m);});
  for(var state:Blocks.OAK_DOOR.getStateDefinition().getPossibleStates())for(var direction:Direction.values())for(int kind=0;kind<5;kind++)for(boolean sturdy:new boolean[]{false,true}){
   var opposite=state.getValue(DoorBlock.HALF)==DoubleBlockHalf.LOWER?DoubleBlockHalf.UPPER:DoubleBlockHalf.LOWER;
   var neighbor=switch(kind){case 0->Blocks.AIR.defaultBlockState();case 1->state;case 2->state.setValue(DoorBlock.HALF,opposite);case 3->Blocks.BIRCH_DOOR.defaultBlockState().setValue(DoorBlock.HALF,opposite).setValue(DoorBlock.FACING,Direction.EAST).setValue(DoorBlock.OPEN,true);default->Blocks.STONE.defaultBlockState();};
   var level=(LevelReader)Proxy.newProxyInstance(LevelReader.class.getClassLoader(),new Class<?>[]{LevelReader.class},(p,m,v)->switch(m.getName()) {case "getBlockState" -> sturdy?Blocks.STONE.defaultBlockState():Blocks.AIR.defaultBlockState();default -> throw new AssertionError("unexpected "+m);});
   var result=(BlockState)method.invoke(Blocks.OAK_DOOR,state,level,scheduled,BlockPos.ZERO,direction,BlockPos.ZERO.relative(direction),neighbor,RandomSource.create(0));
   out.println("D "+describe(state)+" "+direction.getName()+" "+kind+" "+sturdy+" "+describe(result));
  }
 }
}
