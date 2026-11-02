import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.util.random.WeightedList;
import net.minecraft.util.valueproviders.ConstantInt;
import net.minecraft.util.valueproviders.UniformInt;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.levelgen.feature.*;
import net.minecraft.world.level.levelgen.feature.stateproviders.*;
import net.minecraft.world.level.levelgen.placement.*;
import com.mojang.serialization.MapCodec;
import java.util.concurrent.CompletableFuture;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;

public class VegetationPatchOracle {
  static class Generator extends ChunkGenerator {
    Generator(){super(null);}
    protected MapCodec<? extends ChunkGenerator> codec(){throw new UnsupportedOperationException();}
    public void spawnOriginalMobs(WorldGenRegion r){}
    public int getGenDepth(){return 64;}
    public int getSeaLevel(){return 32;}
    public int getMinY(){return 0;}
    public int getBaseHeight(int x,int z,Heightmap.Types t,LevelHeightAccessor a,RandomState r){return 0;}
    public NoiseColumn getBaseColumn(int x,int z,LevelHeightAccessor a,RandomState r){throw new UnsupportedOperationException();}
    public void addDebugScreenInfo(List<String> l,RandomState r,BlockPos p,SamplerContext c){}
    public CompletableFuture<ChunkAccess> buildTerrain(ChunkAccess c,Blender b,RandomState r,StructureManager s,BiomeManager m,WorldGenRegion w,Set<Holder<Biome>> p){throw new UnsupportedOperationException();}
  }
  static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
  @SuppressWarnings("unchecked") public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    for(boolean wet:new boolean[]{false,true})for(boolean ceiling:new boolean[]{false,true})for(long seed:new long[]{0,1,-1,42,987654321}) {
      var blocks=new HashMap<BlockPos,BlockState>();var events=new ArrayList<String>();
      java.util.function.Function<BlockPos,BlockState> at=p->{
        var s=blocks.get(p);if(s!=null)return s;
        int y=p.getY();boolean solid=ceiling?y>=7:y<4;
        if(!solid)return Blocks.AIR.defaultBlockState();
        if((ceiling?y==7:y==3) && Math.floorMod(p.getX()+p.getZ(),5)==0)return Blocks.MOSS_BLOCK.defaultBlockState();
        if(Math.floorMod(p.getX()*3+p.getZ(),11)==0)return Blocks.BEDROCK.defaultBlockState();
        return Blocks.STONE.defaultBlockState();
      };
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
        if(m.getName().equals("getMinY"))return 0;if(m.getName().equals("getHeight"))return 64;
        if(m.getName().equals("getBlockState"))return at.apply((BlockPos)v[0]);
        if(m.getName().equals("isStateAtPosition"))return ((Predicate<BlockState>)v[1]).test(at.apply((BlockPos)v[0]));
        if(m.getName().equals("isEmptyBlock"))return at.apply((BlockPos)v[0]).isAir();
        if(m.getName().equals("setBlock")){var p=((BlockPos)v[0]).immutable();var s=(BlockState)v[1];blocks.put(p,s);String extra=s.hasProperty(BlockStateProperties.WATERLOGGED)?":"+s.getValue(BlockStateProperties.WATERLOGGED):"";events.add("W"+pos(p)+","+BuiltInRegistries.BLOCK.getKey(s.getBlock())+extra);return true;}
        throw new UnsupportedOperationException(m.toString());
      });
      Feature nested=new Feature(){public MapCodec<? extends Feature> codec(){return null;}public boolean place(WorldGenLevel w,ChunkGenerator g,RandomSource r,BlockPos p){int n=r.nextInt(3);events.add("P"+pos(p)+","+n);if(n==0)return false;w.setBlock(p,Blocks.OAK_LEAVES.defaultBlockState(),2);return true;}};
      var placed=new PlacedFeature(Holder.direct(nested),List.of());
      var weighted=WeightedList.<BlockState>builder().add(Blocks.MOSS_BLOCK.defaultBlockState(),2).add(Blocks.CLAY.defaultBlockState(),1);
      var set=HolderSet.direct(Blocks.STONE.builtInRegistryHolder(),Blocks.CLAY.builtInRegistryHolder());
      var ground=Holder.<BlockStateProvider>direct(new WeightedStateProvider(weighted));
      var surface=ceiling?CaveSurface.CEILING:CaveSurface.FLOOR;
      VegetationPatchFeature feature=wet?new WaterloggedVegetationPatchFeature(set,ground,Holder.direct(placed),surface,UniformInt.of(1,2),0.4f,3,0.6f,UniformInt.of(1,2),0.35f):new VegetationPatchFeature(set,ground,Holder.direct(placed),surface,UniformInt.of(1,2),0.4f,3,0.6f,UniformInt.of(1,2),0.35f);
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      boolean result=feature.place(world,new Generator(),random,new BlockPos(4,5,-3));
      out.printf("V %s %s %d %s %s R %d%n",wet,ceiling,seed,result,String.join(";",events),random.nextLong());
    }
  }
}
