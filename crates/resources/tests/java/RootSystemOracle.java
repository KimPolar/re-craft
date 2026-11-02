import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.tags.FluidTags;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.*;
import net.minecraft.world.level.levelgen.feature.stateproviders.*;
import net.minecraft.world.level.levelgen.placement.PlacedFeature;
import com.mojang.serialization.MapCodec;

public class RootSystemOracle {
  static String pos(BlockPos p) { return p.getX()+","+p.getY()+","+p.getZ(); }
  public static void main(String[] args) throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
    bind.invoke(Fluids.WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
    bind.invoke(Fluids.FLOWING_WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
    bind.invoke(Fluids.LAVA.builtInRegistryHolder(),List.of(FluidTags.LAVA));
    bind.invoke(Fluids.FLOWING_LAVA.builtInRegistryHolder(),List.of(FluidTags.LAVA));
    for (int mode=0;mode<5;mode++) for(long seed:new long[]{0,1,-1,42,987654321}) {
      final int m=mode;
      var blocks=new HashMap<BlockPos,BlockState>(); var events=new ArrayList<String>();
      java.util.function.Function<BlockPos,BlockState> at=p->{
        var s=blocks.get(p); if(s!=null)return s;
        if(m==4 && p.getY()==3 && p.getX()==0 && p.getZ()==0)return Blocks.STONE.defaultBlockState();
        if(m==1 && p.getY()==7 && p.getX()==0 && p.getZ()==0)return Blocks.WATER.defaultBlockState();
        if(m==2 && p.getY()==8 && p.getX()==0 && p.getZ()==0)return Blocks.STONE.defaultBlockState();
        return (p.getY()<=2 || p.getY()==4 || p.getY()==5 ? Blocks.STONE : Blocks.AIR).defaultBlockState();
      };
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,method,v)->{
        switch(method.getName()) {
          case "getHeight": return v!=null && v.length==2 ? 6 : 64;
          case "getMinY": return 0;
          case "getBlockState": return at.apply((BlockPos)v[0]);
          case "getFluidState": return at.apply((BlockPos)v[0]).getFluidState();
          case "isEmptyBlock": return at.apply((BlockPos)v[0]).isAir();
          case "setBlock": { var p=((BlockPos)v[0]).immutable();var s=(BlockState)v[1];blocks.put(p,s);events.add("W"+pos(p)+","+BuiltInRegistries.BLOCK.getKey(s.getBlock()));return true; }
        }
        throw new UnsupportedOperationException(method.toString());
      });
      Feature nested=new Feature(){public MapCodec<? extends Feature> codec(){return null;}public boolean place(WorldGenLevel w,ChunkGenerator g,RandomSource r,BlockPos p){int n=r.nextInt(3);events.add("P"+pos(p)+","+n);return m!=3 && n!=0;}};
      var tree=new PlacedFeature(Holder.direct(nested),List.of());
      var feature=new RootSystemFeature(Holder.direct(tree),3,0,0,3,HolderSet.direct(Blocks.STONE.builtInRegistryHolder()),Holder.direct(new SimpleStateProvider(Blocks.ROOTED_DIRT.defaultBlockState())),8,10,3,2,Holder.direct(new SimpleStateProvider(Blocks.HANGING_ROOTS.defaultBlockState())),12,2,BlockPredicate.matchesBlocks(Blocks.AIR));
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      boolean result=feature.place(world,new VegetationPatchOracle.Generator(),random,new BlockPos(0,3,0));
      out.printf("R %d %d %s %s X %d%n",mode,seed,result,String.join(";",events),random.nextLong());
    }
  }
}
