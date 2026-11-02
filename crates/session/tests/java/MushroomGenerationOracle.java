import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.feature.SimpleBlockFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public class MushroomGenerationOracle {
 public static void main(String[] args)throws Exception {
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
  var supports=List.of(Blocks.STONE,Blocks.DIRT,Blocks.GRASS_BLOCK,Blocks.PODZOL,Blocks.MYCELIUM,Blocks.CRIMSON_NYLIUM,Blocks.WARPED_NYLIUM,Blocks.WATER,Blocks.AIR,Blocks.CHEST,Blocks.SPAWNER,Blocks.DEEPSLATE);
  for(var b:List.of(Blocks.BROWN_MUSHROOM,Blocks.RED_MUSHROOM)){
   var s=b.defaultBlockState();out.printf("E %s %s %s %s%n",BuiltInRegistries.BLOCK.getKey(b),s.hasBlockEntity(),net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(s),s.getPostProcessPos(null,BlockPos.ZERO));
  }
  for(var block:List.of(Blocks.BROWN_MUSHROOM,Blocks.RED_MUSHROOM))for(var support:supports)for(boolean override:new boolean[]{false,true})for(int light:new int[]{0,12,13,15}){
   bind.invoke(support.builtInRegistryHolder(),override?List.of(BlockTags.OVERRIDES_MUSHROOM_LIGHT_REQUIREMENT):List.of());
   final int[] calls={0};var writes=new ArrayList<String>();var pos=new BlockPos(8,8,8);
   var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
    if(m.getName().equals("getBlockState"))return ((BlockPos)a[0]).equals(pos.below())?support.defaultBlockState():Blocks.AIR.defaultBlockState();
    if(m.getName().equals("getRawBrightness")){if((int)a[1]!=0)throw new AssertionError("sky subtraction");calls[0]++;return light;}
    if(m.getName().equals("setBlock")){if((int)a[2]!=2)throw new AssertionError("flags");writes.add(BuiltInRegistries.BLOCK.getKey(((BlockState)a[1]).getBlock()).toString());return true;}
    throw new UnsupportedOperationException(m.toString());
   });
   var feature=new SimpleBlockFeature(BlockStateProvider.of(block));var random=new WorldgenRandom(new XoroshiroRandomSource(33));
   boolean placed=feature.place(world,null,random,pos);
   out.printf("C %s %s %s %d %s %d %s %d %s%n",BuiltInRegistries.BLOCK.getKey(block),BuiltInRegistries.BLOCK.getKey(support),override,light,placed,calls[0],writes.isEmpty()?"-":String.join(";",writes),random.nextLong(),support.defaultBlockState().isSolidRender());
   bind.invoke(support.builtInRegistryHolder(),List.of());
  }
 }
}
