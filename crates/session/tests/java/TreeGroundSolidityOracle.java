import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.state.properties.BooleanProperty;
import net.minecraft.world.level.block.state.properties.IntegerProperty;
import java.util.Set;

public class TreeGroundSolidityOracle {
  static final Set<String> PLANTS = Set.of(
    "short_grass","fern","bush","dandelion","poppy","blue_orchid","allium","azure_bluet",
    "red_tulip","orange_tulip","white_tulip","pink_tulip","oxeye_daisy","cornflower",
    "lily_of_the_valley","firefly_bush","closed_eyeblossom","azalea","flowering_azalea",
    "sweet_berry_bush","short_dry_grass","tall_dry_grass","dead_bush","tall_grass",
    "large_fern","sunflower","rose_bush","peony","lilac","oak_sapling","spruce_sapling",
    "birch_sapling","jungle_sapling","acacia_sapling","dark_oak_sapling","cherry_sapling",
    "pale_oak_sapling","seagrass","tall_seagrass","small_dripleaf","brown_mushroom","red_mushroom",
    "moss_carpet","white_carpet","orange_carpet","magenta_carpet","light_blue_carpet","yellow_carpet",
    "lime_carpet","pink_carpet","gray_carpet","light_gray_carpet","cyan_carpet","purple_carpet",
    "blue_carpet","brown_carpet","green_carpet","red_carpet","black_carpet");
  public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    for(String name:args) {
      var state=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+name)).defaultBlockState();
      if((name.equals("air") || name.equals("cave_air") || name.equals("void_air")) && state.useShapeForLightOcclusion())throw new AssertionError("air unexpectedly uses shape occlusion: "+name);
      out.printf("S %s %s %s%n",name,state.isSolidRender(),state.isFaceSturdy(EmptyBlockGetter.INSTANCE,BlockPos.ZERO,Direction.UP));
      if(PLANTS.contains(name)) {
        for(var variant:state.getBlock().getStateDefinition().getPossibleStates()){
          if(variant.isSolidRender())throw new AssertionError("plant renders solid: "+variant);
          for(var direction:Direction.values()){
            boolean expected=((name.equals("azalea") || name.equals("flowering_azalea")) && direction==Direction.UP)
                || (name.endsWith("_carpet") && direction==Direction.DOWN);
            if(variant.isFaceSturdy(EmptyBlockGetter.INSTANCE,BlockPos.ZERO,direction)!=expected)throw new AssertionError("unexpected plant face: "+variant+" "+direction);
          }
        }
        out.printf("P %s", name);
        for(var direction:Direction.values()) out.printf(" %s",state.isFaceSturdy(EmptyBlockGetter.INSTANCE,BlockPos.ZERO,direction));
        out.println();
      }
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
