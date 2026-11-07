import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.TrapDoorBlock;

public class TrapdoorCollisionOracle {
  public static void main(String[] args) {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    for(var block:BuiltInRegistries.BLOCK) if(block instanceof TrapDoorBlock) {
      for(var state:block.getStateDefinition().getPossibleStates()) {
        var shape=state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO);
        var outline=state.getShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO);
        var boxes=shape.toAabbs();var outlines=outline.toAabbs();
        if(boxes.size()!=1||outlines.size()!=1||!boxes.get(0).equals(outlines.get(0))) throw new AssertionError("unexpected shape");
        var b=boxes.get(0);
        out.printf("S %s %s %s %s %s %s %s %s %s %s %s %s %s%n",BuiltInRegistries.BLOCK.getKey(block),state.getValue(TrapDoorBlock.FACING).getName(),state.getValue(TrapDoorBlock.HALF).getSerializedName(),state.getValue(TrapDoorBlock.OPEN),state.getValue(TrapDoorBlock.POWERED),state.getValue(TrapDoorBlock.WATERLOGGED),b.minX,b.minY,b.minZ,b.maxX,b.maxY,b.maxZ,state.getFluidState().isEmpty());
      }
    }
  }
}
