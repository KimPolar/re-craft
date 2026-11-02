import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.SnowAndFreezeFeature;

public class FreezeFeatureOracle {
 static Biome biome(float t,boolean wet){return new Biome.BiomeBuilder().temperature(t).downfall(0.5f).hasPrecipitation(wet).specialEffects(new BiomeSpecialEffects.Builder().waterColor(0).build()).generationSettings(BiomeGenerationSettings.EMPTY).build();}
 static BlockState original(BlockPos p){
  if(p.getY()==9)return switch(Math.floorMod(p.getX()*31+p.getZ()*17,7)){
   case 0 -> Blocks.WATER.defaultBlockState();case 1 -> Blocks.GRASS_BLOCK.defaultBlockState();case 2 -> Blocks.STONE.defaultBlockState();case 3 -> Blocks.AIR.defaultBlockState();case 4 -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,3);case 5 -> Blocks.SNOW.defaultBlockState().setValue(BlockStateProperties.LAYERS,8);default -> Blocks.SNOW.defaultBlockState();};
  if(p.getY()==10){int n=Math.floorMod(p.getX()*7+p.getZ()*3,13);if(n==0)return Blocks.SNOW.defaultBlockState().setValue(BlockStateProperties.LAYERS,2);if(n==1)return Blocks.STONE.defaultBlockState();}
  return Blocks.AIR.defaultBlockState();
 }
 static String name(BlockState s){if(s.is(Blocks.ICE))return "ice";if(s.is(Blocks.SNOW))return "snow"+s.getValue(BlockStateProperties.LAYERS);if(s.is(Blocks.GRASS_BLOCK))return "grass"+s.getValue(BlockStateProperties.SNOWY);throw new AssertionError(s);}
 public static void main(String[] args){
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();var cold=biome(0f,true);var dry=biome(0f,false);var warm=biome(0.8f,true);
  for(int mode=0;mode<5;mode++){
   final int scenario=mode;var changes=new HashMap<BlockPos,BlockState>();var writes=new ArrayList<String>();
   var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
    if(m.getName().equals("getHeight")){int x=(int)v[1],z=(int)v[2];return scenario==4?(Math.floorMod(x+z,2)==0?0:16):10;}
    if(m.getName().equals("getBiome")){var p=(BlockPos)v[0];return Holder.direct(scenario==0?cold:scenario==1?dry:scenario==2?warm:Math.floorMod(p.getX()+p.getZ(),3)==0?warm:Math.floorMod(p.getX()+p.getZ(),3)==1?cold:dry);}
    if(m.getName().equals("getSeaLevel"))return 63;
    if(m.getName().equals("isInsideBuildHeight")){int y=(int)v[0];return y>=0&&y<16;}
    if(m.getName().equals("getBrightness")){var p=(BlockPos)v[1];return Math.floorMod(p.getX()+2*p.getZ(),5)==0?10:9;}
    if(m.getName().equals("getBlockState")){var p=(BlockPos)v[0];return changes.getOrDefault(p,original(p));}
    if(m.getName().equals("getFluidState")){var p=(BlockPos)v[0];return changes.getOrDefault(p,original(p)).getFluidState();}
    if(m.getName().equals("setBlock")){var p=((BlockPos)v[0]).immutable();var s=(BlockState)v[1];if((int)v[2]!=2)throw new AssertionError("flags");changes.put(p,s);writes.add(p.getX()+","+p.getY()+","+p.getZ()+","+name(s));return true;}
    throw new UnsupportedOperationException(m.toString());
   });
   var random=new WorldgenRandom(new XoroshiroRandomSource(123));boolean ok=new SnowAndFreezeFeature().place(world,null,random,new BlockPos(-8,0,3));
   out.printf("C %d %s W %s R %d%n",mode,ok,String.join(";",writes),random.nextLong());
  }
 }
}
