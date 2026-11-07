import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;

public class MonsterRoomBlockOracle {
 public static void main(String[] args){
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var world=(BlockGetter)Proxy.newProxyInstance(BlockGetter.class.getClassLoader(),new Class[]{BlockGetter.class},(p,m,v)->{
   if(m.getName().equals("getBlockState"))return Blocks.AIR.defaultBlockState();
   if(m.getName().equals("getMinY"))return 0;if(m.getName().equals("getHeight"))return 16;
   throw new UnsupportedOperationException(m.toString());
  });
  var names=List.of("air","cave_air","void_air","water","lava","leaf_litter","oak_leaves","dark_oak_leaves","birch_leaves","azalea_leaves","flowering_azalea_leaves","chest","spawner","stone","deepslate","tuff","granite","diorite","andesite","calcite","dirt","grass_block","podzol","mycelium","coarse_dirt","rooted_dirt","mud","packed_mud","moss_block","clay","gravel","sand","red_sand","sandstone","bedrock","cobblestone","mossy_cobblestone","cobbled_deepslate","coal_ore","deepslate_coal_ore","iron_ore","deepslate_iron_ore","copper_ore","deepslate_copper_ore","gold_ore","deepslate_gold_ore","redstone_ore","deepslate_redstone_ore","emerald_ore","deepslate_emerald_ore","lapis_ore","deepslate_lapis_ore","diamond_ore","deepslate_diamond_ore","netherrack","basalt","smooth_basalt","amethyst_block","budding_amethyst","blackstone","ancient_debris");
  var expandedNames=new ArrayList<>(names);
  expandedNames.addAll(List.of("moss_carpet","short_grass","tall_grass","fern","large_fern","azalea","flowering_azalea","oak_log","birch_log","dark_oak_log","brown_mushroom_block","red_mushroom_block","mushroom_stem"));
  expandedNames.addAll(List.of("rail","powered_rail","detector_rail","activator_rail","cobweb","torch","wall_torch","soul_torch","soul_wall_torch","lantern","soul_lantern","oak_slab","oak_stairs"));
  var supportNames=new HashSet<>(List.of("rail","powered_rail","detector_rail","activator_rail","cobweb","torch","wall_torch","soul_torch","soul_wall_torch","lantern","soul_lantern"));
  for(var block:BuiltInRegistries.BLOCK) if(block instanceof net.minecraft.world.level.block.FenceBlock
      || block instanceof net.minecraft.world.level.block.FenceGateBlock
      || block instanceof net.minecraft.world.level.block.DoorBlock
      || block instanceof net.minecraft.world.level.block.IronBarsBlock
      || block instanceof net.minecraft.world.level.block.BedBlock) {
   expandedNames.add(BuiltInRegistries.BLOCK.getKey(block).getPath());
   if(!(block instanceof net.minecraft.world.level.block.DoorBlock)) supportNames.add(BuiltInRegistries.BLOCK.getKey(block).getPath());
  }
  for(var family:List.of("oak","spruce","birch","jungle","acacia","cherry","dark_oak","pale_oak","mangrove","poplar","bamboo","crimson","warped")) {
   var wood=new ArrayList<String>();wood.add(family+"_planks");
   if(family.equals("bamboo")) wood.addAll(List.of("bamboo_block","stripped_bamboo_block"));
   else {
    var nether=family.equals("crimson") || family.equals("warped");
    for(var suffix:nether?List.of("stem","hyphae"):List.of("log","wood"))
     for(var prefix:List.of("","stripped_")) wood.add(prefix+family+"_"+suffix);
   }
   for(var name:wood) {if(!expandedNames.contains(name)) expandedNames.add(name);supportNames.add(name);}
  }
  for(var name:expandedNames){
   var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+name));
   for(var state:block.getStateDefinition().getPossibleStates()){
    var properties=new TreeMap<String,String>();state.getValues().forEach(entry->properties.put(entry.property().getName(),entry.valueName()));
    var pairs=new ArrayList<String>();for(var entry:properties.entrySet())pairs.add(entry.getKey()+"="+entry.getValue());
    var post=state.getPostProcessPos(world,new BlockPos(1,4,1));
    out.printf("C %s %s %s %s %s %s %s%n",name,pairs.isEmpty()?"-":String.join(",",pairs),state.isSolid(),state.isSolidRender(),state.hasBlockEntity(),post!=null,state.liquid());
    if(supportNames.contains(name)) {
     out.printf("S %s %s %s",name,pairs.isEmpty()?"-":String.join(",",pairs),state.isSolidRender());
     for(var direction:Direction.values())out.printf(" %s",state.isFaceSturdy(world,new BlockPos(1,4,1),direction));
     out.println();
    }
   }
  }
 }
}
