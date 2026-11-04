import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.levelgen.blockpredicates.SolidPredicate;

public class GenerationSolidPredicateOracle {
  public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var names=List.of("leaf_litter","seagrass","tall_seagrass","short_grass","fern","bush","dandelion","poppy","blue_orchid","allium","azure_bluet","red_tulip","orange_tulip","white_tulip","pink_tulip","oxeye_daisy","cornflower","lily_of_the_valley","firefly_bush","closed_eyeblossom","sweet_berry_bush","short_dry_grass","tall_dry_grass","dead_bush","sunflower","tall_grass","large_fern","rose_bush","peony","lilac","melon","pumpkin","brown_mushroom","red_mushroom","red_mushroom_block","brown_mushroom_block","mushroom_stem","oak_log","dark_oak_log","oak_leaves","dark_oak_leaves","azalea_leaves","flowering_azalea_leaves","cave_vines","cave_vines_plant","big_dripleaf","big_dripleaf_stem","small_dripleaf","glow_lichen","coal_ore","deepslate_coal_ore","iron_ore","deepslate_iron_ore","copper_ore","deepslate_copper_ore","gold_ore","deepslate_gold_ore","redstone_ore","deepslate_redstone_ore","emerald_ore","deepslate_emerald_ore","lapis_ore","deepslate_lapis_ore","diamond_ore","deepslate_diamond_ore","nether_gold_ore","nether_quartz_ore");
    names=new ArrayList<>(names); names.addAll(List.of("smooth_basalt","amethyst_block","budding_amethyst","small_amethyst_bud","medium_amethyst_bud","large_amethyst_bud","amethyst_cluster"));
    var pos=new BlockPos(8,8,8); var target=pos.offset(1,-2,3);
    for(var name:names){
      var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+name));
      for(var state:block.getStateDefinition().getPossibleStates()){
        var world=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(o,m,a)->{
          if(m.getName().equals("getBlockState") && target.equals(a[0]))return state;
          throw new AssertionError(m.toString());
        });
        var props=new TreeMap<String,String>();state.getValues().forEach(v->props.put(v.property().getName(),v.valueName()));
        var pairs=new ArrayList<String>();props.forEach((k,v)->pairs.add(k+"="+v));
        boolean solid=new SolidPredicate(new Vec3i(1,-2,3)).test(world,pos);
        if(solid!=state.isSolid())throw new AssertionError("predicate differs from state");
        out.printf("S %s %s %s%n",name,pairs.isEmpty()?"-":String.join(",",pairs),solid);
        if(name.endsWith("_ore"))out.printf("O %s %s %s%n",name,pairs.isEmpty()?"-":String.join(",",pairs),state.getFluidState().isEmpty());
        if(name.equals("glow_lichen")){
          out.printf("G %s %s",String.join(",",pairs),state.isSolidRender());
          for(var direction:Direction.values())out.printf(" %s",state.isFaceSturdy(EmptyBlockGetter.INSTANCE,pos,direction));
          out.println();
        }
      }
    }
  }
}
