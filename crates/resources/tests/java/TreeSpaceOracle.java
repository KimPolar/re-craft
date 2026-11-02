import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.*;
import java.util.*;
import java.util.function.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.util.valueproviders.ConstantInt;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.TreeFeature;
import net.minecraft.world.level.levelgen.feature.featuresize.FeatureSize;
import net.minecraft.world.level.levelgen.feature.foliageplacers.BlobFoliagePlacer;
import net.minecraft.world.level.levelgen.feature.foliageplacers.FoliagePlacer;
import net.minecraft.world.level.levelgen.feature.trunkplacers.StraightTrunkPlacer;

public class TreeSpaceOracle {
  static int mode, minY, maxY, used;
  static BlockState testState;
  static List<String> calls;
  static int kind(BlockPos p) {
    if(mode==1 && p.equals(new BlockPos(3,20,-4)))return 1;
    if(mode==2 && p.equals(new BlockPos(2,23,-5)))return 1;
    if(mode==3 && p.equals(new BlockPos(3,22,-4)))return 2;
    if(mode==4 && p.equals(new BlockPos(4,26,-3)))return 1;
    if(mode==5 && Math.floorMod(p.getX()*3+p.getY()*5+p.getZ()*7,37)==0)return 1;
    return 0;
  }
  static String at(String prefix,BlockPos p){return prefix+p.getX()+","+p.getY()+","+p.getZ();}
  static class Trunk extends StraightTrunkPlacer {
    Trunk(int height){super(height,0,0);}
    @Override public boolean isFree(WorldGenLevel w,BlockPos p){calls.add(at("F",p));return kind(p)!=1;}
    @Override public List<FoliagePlacer.FoliageAttachment> placeTrunk(WorldGenLevel w,BiConsumer<BlockPos,BlockState> s,RandomSource r,int h,BlockPos p,TreeFeature t){used=h;return List.of();}
  }
  @SuppressWarnings("unchecked")
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,method,values)->{
      if(method.getName().equals("getMinY"))return minY;
      if(method.getName().equals("getMaxY"))return maxY;
      if(method.getName().equals("isStateAtPosition")){
        var p=(BlockPos)values[0];calls.add(at("V",p));
        return ((Predicate<BlockState>)values[1]).test(testState!=null?testState:(kind(p)==2?Blocks.VINE:Blocks.AIR).defaultBlockState());
      }
      throw new UnsupportedOperationException(method.toString());
    });
    var method=TreeFeature.class.getDeclaredMethod("doPlace",WorldGenLevel.class,RandomSource.class,BlockPos.class,BiConsumer.class,BiConsumer.class,FoliagePlacer.FoliageSetter.class);
    method.setAccessible(true);
    var setter=new FoliagePlacer.FoliageSetter(){public void set(BlockPos p,BlockState s){} public boolean isSet(BlockPos p){return false;}};
    BiConsumer<BlockPos,BlockState> noWrite=(p,s)->{};
    String[] defs={"{\"type\":\"minecraft:two_layers_feature_size\"}","{\"type\":\"minecraft:two_layers_feature_size\",\"limit\":2,\"lower_size\":1,\"upper_size\":2,\"min_clipped_height\":0}","{\"type\":\"minecraft:three_layers_feature_size\",\"limit\":2,\"upper_limit\":3,\"lower_size\":0,\"middle_size\":2,\"upper_size\":1,\"min_clipped_height\":4}"};
    for(int si=0;si<defs.length;si++)for(int height:new int[]{0,4,6,12})for(boolean ignore:new boolean[]{false,true})for(mode=0;mode<6;mode++)for(int bound=0;bound<3;bound++){
      minY=bound==1?20:0;maxY=bound==2?22:255;used=-999;calls=new ArrayList<>();
      FeatureSize size=FeatureSize.CODEC.parse(JsonOps.INSTANCE,JsonParser.parseString(defs[si])).getOrThrow();
      var blob=new BlobFoliagePlacer(ConstantInt.of(2),ConstantInt.of(0),3);
      var tree=new TreeFeature(null,new Trunk(height),null,blob,Optional.empty(),size,List.of(),ignore,null);
      var random=new WorldgenRandom(new XoroshiroRandomSource(0));
      boolean ok=(boolean)method.invoke(tree,world,random,new BlockPos(3,20,-4),noWrite,noWrite,setter);
      out.printf("C %d %d %s %d %d %s %d %s%n",si,height,ignore,mode,bound,ok,used,String.join(";",calls));
    }
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.OAK_LOG.builtInRegistryHolder(),List.of(BlockTags.LOGS));
    bind.invoke(Blocks.VINE.builtInRegistryHolder(),List.of(BlockTags.REPLACEABLE_BY_TREES));
    bind.invoke(Blocks.SHORT_GRASS.builtInRegistryHolder(),List.of(BlockTags.REPLACEABLE_BY_TREES));
    for(var block:List.of(Blocks.AIR,Blocks.STONE,Blocks.OAK_LOG,Blocks.VINE,Blocks.SHORT_GRASS,Blocks.OAK_LEAVES)){
      testState=block.defaultBlockState();calls=new ArrayList<>();var pos=new BlockPos(3,20,-4);
      out.printf("B %s %s %s%n",BuiltInRegistries.BLOCK.getKey(block),new StraightTrunkPlacer(4,2,0).isFree(world,pos),TreeFeature.isVine(world,pos));
    }
  }
}
