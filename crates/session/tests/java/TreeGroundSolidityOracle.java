import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.EmptyBlockGetter;

public class TreeGroundSolidityOracle {
  public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    for(String name:args) {
      var state=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+name)).defaultBlockState();
      out.printf("S %s %s %s%n",name,state.isSolidRender(),state.isFaceSturdy(EmptyBlockGetter.INSTANCE,BlockPos.ZERO,Direction.UP));
    }
  }
}
