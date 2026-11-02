import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.SnowAndFreezeFeature;

public class FreezeChunkOracle {
 static BlockState initial(BlockPos p){
  if(p.getY()==7)return switch(Math.floorMod(p.getX()+p.getZ()*3,4)){case 0->Blocks.STONE.defaultBlockState();case 1->Blocks.GRASS_BLOCK.defaultBlockState();case 2->Blocks.WATER.defaultBlockState();default->Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,3);};
  if(p.getY()==8 && Math.floorMod(p.getX()*7+p.getZ()*3,17)==0)return Blocks.SNOW.defaultBlockState().setValue(BlockStateProperties.LAYERS,2);
  return (p.getY()<7?Blocks.STONE:Blocks.AIR).defaultBlockState();
 }
 static String name(BlockState s){if(s.is(Blocks.ICE))return "ice";if(s.is(Blocks.WATER))return "water"+s.getValue(LiquidBlock.LEVEL);if(s.is(Blocks.GRASS_BLOCK))return "grass"+s.getValue(BlockStateProperties.SNOWY);if(s.is(Blocks.STONE))return "stone";if(s.is(Blocks.SNOW))return "snow"+s.getValue(BlockStateProperties.LAYERS);if(s.isAir())return "air";throw new AssertionError(s);}
 public static void main(String[] args)throws Exception {
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);bind.invoke(Blocks.ICE.builtInRegistryHolder(),List.of(BlockTags.CANNOT_SUPPORT_SNOW_LAYER));
  var biome=new Biome.BiomeBuilder().temperature(0f).downfall(0.5f).hasPrecipitation(true).specialEffects(new BiomeSpecialEffects.Builder().waterColor(0).build()).generationSettings(BiomeGenerationSettings.EMPTY).build();
  var changes=new HashMap<BlockPos,BlockState>();
  var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
   if(m.getName().equals("getHeight"))return 8;
   if(m.getName().equals("getBiome"))return Holder.direct(biome);
   if(m.getName().equals("getSeaLevel"))return 8;
   if(m.getName().equals("isInsideBuildHeight")){int y=(int)v[0];return y>=0&&y<16;}
   if(m.getName().equals("getBrightness")){var p=(BlockPos)v[1];return Math.floorMod(p.getX()+2*p.getZ(),5)==0?10:9;}
   if(m.getName().equals("getBlockState")){var p=(BlockPos)v[0];return changes.getOrDefault(p,initial(p));}
   if(m.getName().equals("getFluidState")){var p=(BlockPos)v[0];return changes.getOrDefault(p,initial(p)).getFluidState();}
   if(m.getName().equals("setBlock")){var p=((BlockPos)v[0]).immutable();if((int)v[2]!=2)throw new AssertionError("flags");changes.put(p,(BlockState)v[1]);return true;}
   throw new UnsupportedOperationException(m.toString());
  });
  boolean ok=new SnowAndFreezeFeature().place(world,null,new WorldgenRandom(new XoroshiroRandomSource(1)),new BlockPos(0,0,0));
  for(var entry:changes.entrySet())if(entry.getValue().getPostProcessPos(world,entry.getKey())!=null)throw new AssertionError("unexpected postprocessing "+entry);
  var results=new ArrayList<String>();for(int x=0;x<16;x++)for(int z=0;z<16;z++){var below=new BlockPos(x,7,z);var above=below.above();results.add(x+","+z+","+name(changes.getOrDefault(below,initial(below)))+","+name(changes.getOrDefault(above,initial(above))));}
  out.println("RESULT "+ok+" "+String.join(";",results));
 }
}
