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
  var names=List.of("air","cave_air","void_air","water","lava","chest","spawner","stone","deepslate","tuff","granite","diorite","andesite","calcite","dirt","grass_block","podzol","mycelium","coarse_dirt","clay","gravel","sand","red_sand","sandstone","bedrock","cobblestone","mossy_cobblestone","cobbled_deepslate","coal_ore","deepslate_coal_ore","iron_ore","deepslate_iron_ore","copper_ore","deepslate_copper_ore","gold_ore","deepslate_gold_ore","redstone_ore","deepslate_redstone_ore","emerald_ore","deepslate_emerald_ore","lapis_ore","deepslate_lapis_ore","diamond_ore","deepslate_diamond_ore","netherrack","basalt","smooth_basalt","amethyst_block","budding_amethyst","blackstone","ancient_debris");
  for(var name:names){
   var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+name));
   for(var state:block.getStateDefinition().getPossibleStates()){
    var properties=new TreeMap<String,String>();state.getValues().forEach(entry->properties.put(entry.property().getName(),entry.valueName()));
    var pairs=new ArrayList<String>();for(var entry:properties.entrySet())pairs.add(entry.getKey()+"="+entry.getValue());
    var post=state.getPostProcessPos(world,new BlockPos(1,4,1));
    out.printf("C %s %s %s %s %s %s%n",name,pairs.isEmpty()?"-":String.join(",",pairs),state.isSolid(),state.isSolidRender(),state.hasBlockEntity(),post!=null);
   }
  }
 }
}
