import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.util.valueproviders.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.*;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.placement.*;
import net.minecraft.world.level.levelgen.presets.WorldPresets;

public class SelectorOracle {
  static PlacedFeature leaf(Block block,int count,boolean filter) {
    var mods=new ArrayList<PlacementModifier>();mods.add(CountPlacement.of(count));
    mods.add(new OffsetPlacement(UniformInt.of(-2,2),ConstantInt.of(0),UniformInt.of(-2,2)));
    if(filter)mods.add(BlockPredicateFilter.forPredicate(BlockPredicate.matchesBlocks(Blocks.AIR)));
    return new PlacedFeature(Holder.direct(new SimpleBlockFeature(BlockStateProvider.of(block))),mods);
  }
  static PlacedFeature root(int kind) {
    var a=leaf(Blocks.MELON,3,true);var b=leaf(Blocks.PUMPKIN,2,false);
    var weighted=new RandomSelectorFeature(List.of(new WeightedPlacedFeature(Holder.direct(a),0.25f),new WeightedPlacedFeature(Holder.direct(b),0.6f)),Holder.direct(a));
    var inner=new PlacedFeature(Holder.direct(weighted),List.of(CountPlacement.of(2)));
    Feature chosen=kind==0?new SimpleRandomSelectorFeature(HolderSet.direct(Holder.direct(a),Holder.direct(b),Holder.direct(inner))):kind==1?new RandomSelectorFeature(List.of(new WeightedPlacedFeature(Holder.direct(inner),0.4f),new WeightedPlacedFeature(Holder.direct(b),0.3f)),Holder.direct(a)):new RandomBooleanSelectorFeature(Holder.direct(inner),Holder.direct(b));
    return new PlacedFeature(Holder.direct(chosen),List.of(CountPlacement.of(2)));
  }
  static BlockState initial(int x,int z,int profile){return profile==1&&Math.floorMod(x*7+z*3,5)==0?Blocks.STONE.defaultBlockState():Blocks.AIR.defaultBlockState();}
  public static void main(String[]args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var generator=WorldPresets.getNormalOverworld(VanillaRegistries.createWorldLookup()).generator();
    for(int kind=0;kind<3;kind++)for(int profile=0;profile<2;profile++)for(int limited=0;limited<2;limited++)for(long seed=0;seed<16;seed++) {
      var map=new HashMap<BlockPos,BlockState>();int terrain=profile,restriction=limited;
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
        if(m.getName().equals("getMinY"))return 0;if(m.getName().equals("getHeight"))return 16;
        var p=(BlockPos)a[0];var old=map.getOrDefault(p,initial(p.getX(),p.getZ(),terrain));
        return switch(m.getName()) {
          case "getBlockState"->old;
          case "isStateAtPosition"->((Predicate<BlockState>)a[1]).test(old);
          case "setBlock"->{if(restriction==0)map.put(p.immutable(),(BlockState)a[1]);yield restriction==0;}
          default->throw new UnsupportedOperationException(m.getName());
        };
      });
      if(kind==0&&profile==0&&limited==0&&seed==0){
        var child=new PlacedFeature(Holder.direct(new SimpleBlockFeature(BlockStateProvider.of(Blocks.MELON))),List.of(BiomeFilter.biome()));
        var selector=new SimpleRandomSelectorFeature(HolderSet.direct(Holder.direct(child)));
        try {selector.place(world,generator,new WorldgenRandom(new XoroshiroRandomSource(0)),new BlockPos(8,8,8));throw new AssertionError("nested biome accepted");}
        catch(IllegalStateException expected){out.println("N rejected");}
      }
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));boolean result=root(kind).place(world,generator,random,new BlockPos(8,8,8));
      out.printf("C %d %d %d %d %s %d%n",kind,profile,limited,seed,result,random.nextLong());
      for(int z=6;z<=10;z++)for(int x=6;x<=10;x++){var p=new BlockPos(x,8,z);var s=map.getOrDefault(p,initial(x,z,profile));if(s!=initial(x,z,profile))out.printf("B %d %d %s%n",x,z,BuiltInRegistries.BLOCK.getKey(s.getBlock()));}
    }
  }
}
