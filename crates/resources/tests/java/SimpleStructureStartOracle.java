import com.mojang.serialization.MapCodec;
import java.util.*;
import java.util.concurrent.CompletableFuture;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.core.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.structures.*;

public class SimpleStructureStartOracle {
  static class Generator extends ChunkGenerator {
    int base;
    Generator(BiomeSource source,int base){super(source);this.base=base;}
    protected MapCodec<? extends ChunkGenerator> codec(){throw new UnsupportedOperationException();}
    public void spawnOriginalMobs(WorldGenRegion r){}
    public int getGenDepth(){return 384;}
    public int getSeaLevel(){return 63;}
    public int getMinY(){return -64;}
    public int getBaseHeight(int x,int z,Heightmap.Types t,LevelHeightAccessor a,RandomState r){return getFirstOccupiedHeight(x,z,t,a,r)+1;}
    public int getFirstOccupiedHeight(int x,int z,Heightmap.Types t,LevelHeightAccessor a,RandomState r){return base+Math.floorMod(x*13+z*7,11)-5-(t==Heightmap.Types.OCEAN_FLOOR_WG?10:0);}
    public NoiseColumn getBaseColumn(int x,int z,LevelHeightAccessor a,RandomState r){throw new UnsupportedOperationException();}
    public void addDebugScreenInfo(List<String> l,RandomState r,BlockPos p,SamplerContext c){}
    public CompletableFuture<ChunkAccess> buildTerrain(ChunkAccess c,Blender b,RandomState r,StructureManager s,BiomeManager m,WorldGenRegion w,Set<Holder<Biome>> p){throw new UnsupportedOperationException();}
  }
  public static void main(String[] args)throws Exception{
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var good=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));var bad=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));
    var source=new FixedBiomeSource(good);var settings=new Structure.StructureSettings(HolderSet.direct(good));
    Structure[] types={new DesertPyramidStructure(settings),new JungleTempleStructure(settings),new SwampHutStructure(settings),new BuriedTreasureStructure(settings)};
    long[] seeds={0,1,-1,123456789L,Long.MIN_VALUE,Long.MAX_VALUE};
    for(int kind=0;kind<types.length;kind++)for(long seed:seeds)for(int index=0;index<31;index++)for(int mode=0;mode<4;mode++){
      int x=index*17-251,z=193-index*13,base=mode==1?60:75;
      var generator=new Generator(source,base);var random=new WorldgenRandom(new LegacyRandomSource(0));random.setLargeFeatureSeed(seed,x,z);
      final int m=mode;BiomeResolver resolver=(qx,qy,qz)->m==3?bad:good;
      var ctx=new Structure.GenerationContext(null,generator,source,null,resolver,null,null,random,seed,new ChunkPos(x,z),LevelHeightAccessor.create(-64,384),h->m!=2&&h==good);
      var stub=types[kind].findValidGenerationPoint(ctx);
      String prefix="S "+kind+" "+seed+" "+x+" "+z+" "+mode+" ";
      if(stub.isEmpty()){out.println(prefix+"X");continue;}
      var s=stub.get();var piece=s.getPiecesBuilder().build().pieces().get(0);var box=piece.getBoundingBox();var pos=s.position();
      out.printf("%sP %d %d %d %s %d %d %d %d %d %d%n",prefix,pos.getX(),pos.getY(),pos.getZ(),piece.getOrientation()==null?"NONE":piece.getOrientation().name(),box.minX(),box.minY(),box.minZ(),box.maxX(),box.maxY(),box.maxZ());
    }
  }
}
