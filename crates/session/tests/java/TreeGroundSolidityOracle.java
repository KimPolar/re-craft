import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.state.properties.BooleanProperty;
import net.minecraft.world.level.block.state.properties.IntegerProperty;

public class TreeGroundSolidityOracle {
  public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    for(String name:args) {
      var state=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+name)).defaultBlockState();
      if((name.equals("air") || name.equals("cave_air") || name.equals("void_air")) && state.useShapeForLightOcclusion())throw new AssertionError("air unexpectedly uses shape occlusion: "+name);
      out.printf("S %s %s %s%n",name,state.isSolidRender(),state.isFaceSturdy(EmptyBlockGetter.INSTANCE,BlockPos.ZERO,Direction.UP));
      if(name.endsWith("_leaves")) {
        for(var variant:state.getBlock().getStateDefinition().getPossibleStates()) if(variant.isSolid()!=state.isSolid())throw new AssertionError("solid varies: "+variant);
        out.printf("L %s %s%n",name,state.isSolid());
      }
      if(name.equals("cave_vines") || name.equals("cave_vines_plant")) {
        var berry=(BooleanProperty)state.getBlock().getStateDefinition().getProperty("berries");
        var age=(IntegerProperty)state.getBlock().getStateDefinition().getProperty("age");
        for(var variant:state.getBlock().getStateDefinition().getPossibleStates()) {
          if(variant.useShapeForLightOcclusion())throw new AssertionError("vine unexpectedly uses shape occlusion: "+variant);
          out.printf("V %s %s %s %s",name,age==null ? -1 : variant.getValue(age),variant.getValue(berry),variant.isSolidRender());
          for(var direction:Direction.values())out.printf(" %s",variant.isFaceSturdy(EmptyBlockGetter.INSTANCE,BlockPos.ZERO,direction));
          out.println();
        }
      }
      if(name.endsWith("_ore")) {
        var lit=(BooleanProperty)state.getBlock().getStateDefinition().getProperty("lit");
        for(var variant:state.getBlock().getStateDefinition().getPossibleStates()) {
          out.printf("R %s %s %s",name,lit==null ? "-" : variant.getValue(lit),variant.isSolidRender());
          for(var direction:Direction.values())out.printf(" %s",variant.isFaceSturdy(EmptyBlockGetter.INSTANCE,BlockPos.ZERO,direction));
          out.println();
        }
      }
    }
  }
}
