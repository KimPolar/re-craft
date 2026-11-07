import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.VinesFeature;

public class GenerationMultifaceSupportOracle {
  public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var names=new ArrayList<>(List.of("moss_block","dirt","coarse_dirt","rooted_dirt","clay","gravel","sand","red_sand","sandstone","red_sandstone","ice","packed_ice","blue_ice","snow_block","magma_block","netherrack","blackstone","grass_block","oak_log","dark_oak_log","oak_leaves","dark_oak_leaves","azalea_leaves","flowering_azalea_leaves","red_mushroom_block","brown_mushroom_block","mushroom_stem","leaf_litter","cave_vines","cave_vines_plant","big_dripleaf","big_dripleaf_stem","small_dripleaf","glow_lichen","short_grass","fern","bush","dandelion","poppy","blue_orchid","allium","azure_bluet","red_tulip","orange_tulip","white_tulip","pink_tulip","oxeye_daisy","cornflower","lily_of_the_valley","firefly_bush","closed_eyeblossom","sweet_berry_bush","short_dry_grass","tall_dry_grass","dead_bush","sunflower","tall_grass","large_fern","rose_bush","peony","lilac","seagrass","tall_seagrass","spore_blossom"));
    names.addAll(List.of("oak_fence","spruce_fence","birch_fence","jungle_fence","acacia_fence","dark_oak_fence","mangrove_fence","cherry_fence","pale_oak_fence","bamboo_fence","crimson_fence","warped_fence","poplar_fence","nether_brick_fence"));
    names.addAll(List.of("oak_planks","spruce_planks","birch_planks","jungle_planks","acacia_planks","dark_oak_planks","mangrove_planks","cherry_planks","pale_oak_planks","bamboo_planks","crimson_planks","warped_planks","poplar_planks","torch","soul_torch","wall_torch","soul_wall_torch","rail","powered_rail","detector_rail","activator_rail"));
    for(var family:List.of("oak","spruce","birch","jungle","acacia","dark_oak","mangrove","cherry","pale_oak","poplar"))
      for(var suffix:List.of("_log","_wood"))for(var prefix:List.of("","stripped_")) {
        var name=prefix+family+suffix;if(!names.contains(name))names.add(name);
      }
    for(var family:List.of("crimson","warped"))
      for(var suffix:List.of("_stem","_hyphae"))for(var prefix:List.of("","stripped_"))names.add(prefix+family+suffix);
    names.addAll(List.of("bamboo_block","stripped_bamboo_block"));
    var pos=new BlockPos(8,8,8);
    for(var name:names){
      var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+name));
      for(var state:block.getStateDefinition().getPossibleStates()){
        var getter=(BlockGetter)Proxy.newProxyInstance(BlockGetter.class.getClassLoader(),new Class[]{BlockGetter.class},(o,m,a)->{
          if(m.getName().equals("getBlockState") && pos.equals(a[0]))return state;
          throw new AssertionError(m.toString());
        });
        var props=new TreeMap<String,String>();state.getValues().forEach(v->props.put(v.property().getName(),v.valueName()));
        var pairs=new ArrayList<String>();props.forEach((k,v)->pairs.add(k+"="+v));
        for(var direction:Direction.values()){
          boolean a=MultifaceBlock.canAttachTo(getter,direction,pos,state);
          boolean b=VineBlock.isAcceptableNeighbour(getter,pos,direction);
          if(a!=b)throw new AssertionError("vine support differs");
          out.printf("S %s %s %s %s%n",name,pairs.isEmpty()?"-":String.join(",",pairs),direction.getSerializedName(),a);
        }
      }
    }
    var faces=new Direction[]{Direction.UP,Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
    for(int mask=0;mask<32;mask++)for(boolean occupied:new boolean[]{false,true}){
      final int bits=mask;var writes=new ArrayList<String>();
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
        if(m.getName().equals("isEmptyBlock"))return !occupied;
        if(m.getName().equals("getBlockState")){
          for(int i=0;i<faces.length;i++)if(pos.relative(faces[i]).equals(a[0]) && (bits&(1<<i))!=0)return Blocks.MOSS_BLOCK.defaultBlockState();
          return Blocks.AIR.defaultBlockState();
        }
        if(m.getName().equals("setBlock")){
          if((int)a[2]!=2)throw new AssertionError("flags");var s=(net.minecraft.world.level.block.state.BlockState)a[1];
          writes.add(s.getValues().map(v->v.property().getName()+"="+v.valueName()).sorted().reduce((x,y)->x+","+y).orElse(""));return true;
        }
        throw new AssertionError(m.toString());
      });
      var random=new WorldgenRandom(new XoroshiroRandomSource(42));
      boolean result=new VinesFeature().place(world,null,random,pos);
      out.printf("V %d %s %s W %s R %d%n",mask,occupied,result,String.join(";",writes),random.nextLong());
    }
  }
}
