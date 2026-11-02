import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.levelgen.blockpredicates.SolidPredicate;

public class GenerationSolidPredicateOracle {
  public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var names=List.of("leaf_litter","seagrass","tall_seagrass","short_grass","fern","bush","dandelion","poppy","blue_orchid","allium","azure_bluet","red_tulip","orange_tulip","white_tulip","pink_tulip","oxeye_daisy","cornflower","lily_of_the_valley","firefly_bush","closed_eyeblossom","sweet_berry_bush","short_dry_grass","tall_dry_grass","dead_bush","sunflower","tall_grass","large_fern","rose_bush","peony","lilac","melon","pumpkin","brown_mushroom","red_mushroom","red_mushroom_block","brown_mushroom_block","mushroom_stem","oak_log","dark_oak_log","oak_leaves","dark_oak_leaves","azalea_leaves","flowering_azalea_leaves");
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
      }
    }
  }
}
