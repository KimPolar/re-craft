import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.feature.UnderwaterMagmaFeature;

public class UnderwaterMagmaOracle {
  static BlockState initial(int profile, BlockPos p) {
    int x=p.getX(),y=p.getY(),z=p.getZ();
    if(profile==0)return Blocks.STONE.defaultBlockState();
    int floor=Math.floorMod(x*3+z*7+profile,3)-1;
    if(y>4)return Blocks.AIR.defaultBlockState();
    if(y>floor)return Blocks.WATER.defaultBlockState();
    if(profile>=3 && y<floor && Math.floorMod(x*7+y*11+z*3+profile,9)==0)return Blocks.AIR.defaultBlockState();
    return (profile>=2 && Math.floorMod(x+y+z,3)==0 ? Blocks.DIRT : Blocks.STONE).defaultBlockState();
  }
  public static void main(String[]args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    String[] names={"air","cave_air","void_air","water","lava","stone","deepslate","netherrack","basalt","blackstone","ancient_debris","dirt","grass_block","mud","clay","sand","sandstone","gravel","snow_block","podzol","coarse_dirt","mycelium","tuff","calcite","granite","diorite","andesite","bedrock","red_sand","moss_block","magma_block","coal_ore","deepslate_coal_ore","iron_ore","deepslate_iron_ore","copper_ore","deepslate_copper_ore","gold_ore","deepslate_gold_ore","redstone_ore","deepslate_redstone_ore","emerald_ore","deepslate_emerald_ore","lapis_ore","deepslate_lapis_ore","diamond_ore","deepslate_diamond_ore","nether_gold_ore","nether_quartz_ore"};
    for(String name:names){var b=BuiltInRegistries.BLOCK.getValue(net.minecraft.resources.Identifier.parse("minecraft:"+name));for(var s:b.getStateDefinition().getPossibleStates())for(var d:Direction.values())out.printf("O minecraft:%s %s%n",name,Block.isShapeFullBlock(s.getFaceOcclusionShape(d)));}
    for(var b:List.of(Blocks.CINNABAR,Blocks.SULFUR))for(var s:b.getStateDefinition().getPossibleStates())for(var d:Direction.values()){
      out.printf("O %s %s%n",BuiltInRegistries.BLOCK.getKey(b),Block.isShapeFullBlock(s.getFaceOcclusionShape(d)));
      out.printf("Q %s %s%n",BuiltInRegistries.BLOCK.getKey(b),s.isFaceSturdy(EmptyBlockGetter.INSTANCE,BlockPos.ZERO,d));
    }
    for(String name:new String[]{"cave_vines","cave_vines_plant"}){var b=BuiltInRegistries.BLOCK.getValue(net.minecraft.resources.Identifier.parse("minecraft:"+name));for(var s:b.getStateDefinition().getPossibleStates())for(var d:Direction.values())out.printf("O minecraft:%s %s%n",name,Block.isShapeFullBlock(s.getFaceOcclusionShape(d)));}
    var magma=Blocks.MAGMA_BLOCK.defaultBlockState();out.println("E "+magma.hasBlockEntity()+" "+PoiTypes.hasPoi(magma)+" "+magma.getPostProcessPos(null,BlockPos.ZERO));
    int number=0;
    for(int profile=0;profile<5;profile++)for(int range:new int[]{0,1,3,5})for(int radius:new int[]{0,1,2})for(float probability:new float[]{0,0.5f,1})for(int restricted=0;restricted<2;restricted++)for(long seed:new long[]{0,7})for(int y:new int[]{-1,1,4}) {
      int code=number++;var changes=new LinkedHashMap<BlockPos,BlockState>();var marks=new ArrayList<BlockPos>();int restriction=restricted;int currentProfile=profile;
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
        var p=(BlockPos)a[0];var s=changes.getOrDefault(p,initial(currentProfile,p));
        return switch(m.getName()){
          case "getBlockState" -> s;
          case "isStateAtPosition" -> ((Predicate<BlockState>)a[1]).test(s);
          case "setBlock" -> {if(restriction==0){var state=(BlockState)a[1];changes.put(p.immutable(),state);var mark=state.getPostProcessPos(null,p);if(mark!=null)marks.add(mark.immutable());}yield restriction==0;}
          default -> throw new UnsupportedOperationException(m.getName());
        };
      });
      var feature=new UnderwaterMagmaFeature(range,radius,probability);
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      boolean result=feature.place(world,null,random,new BlockPos(0,y,0));
      out.printf("C %d %d %d %d %s %d %d %d %s %d%n",code,profile,range,radius,probability,restricted,seed,y,result,random.nextLong());
      for(var p:changes.keySet())out.printf("B %d %d %d%n",p.getX(),p.getY(),p.getZ());
      for(var p:marks)out.printf("M %d %d %d%n",p.getX(),p.getY(),p.getZ());
    }
  }
}
