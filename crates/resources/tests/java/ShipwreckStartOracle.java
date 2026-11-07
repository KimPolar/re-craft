import com.mojang.serialization.MapCodec;
import java.lang.reflect.*;
import java.util.*;
import java.util.concurrent.CompletableFuture;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.core.*;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.structures.*;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class ShipwreckStartOracle {
  static class Generator extends ChunkGenerator {
    int hash=0, calls=0;
    Generator(BiomeSource source){super(source);}
    protected MapCodec<? extends ChunkGenerator> codec(){throw new UnsupportedOperationException();}
    public void spawnOriginalMobs(WorldGenRegion r){}
    public int getGenDepth(){return 384;}
    public int getSeaLevel(){return 63;}
    public int getMinY(){return -64;}
    public int getBaseHeight(int x,int z,Heightmap.Types t,LevelHeightAccessor a,RandomState r){return getFirstOccupiedHeight(x,z,t,a,r)+1;}
    public int getFirstOccupiedHeight(int x,int z,Heightmap.Types t,LevelHeightAccessor a,RandomState r){
      calls++; hash=31*hash+x; hash=31*hash+z; hash=31*hash+(t==Heightmap.Types.WORLD_SURFACE_WG?1:2);
      return -80+Math.floorMod(x*13+z*7,201)-(t==Heightmap.Types.OCEAN_FLOOR_WG?10:0);
    }
    public NoiseColumn getBaseColumn(int x,int z,LevelHeightAccessor a,RandomState r){throw new UnsupportedOperationException();}
    public void addDebugScreenInfo(List<String> l,RandomState r,BlockPos p,SamplerContext c){}
    public CompletableFuture<ChunkAccess> buildTerrain(ChunkAccess c,Blender b,RandomState r,StructureManager s,BiomeManager m,WorldGenRegion w,Set<Holder<Biome>> p){throw new UnsupportedOperationException();}
  }
  public static void main(String[] args)throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var good=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));var source=new FixedBiomeSource(good);
    var settings=new Structure.StructureSettings(HolderSet.direct(good));
    var manager=(StructureTemplateManager)unsafe.allocateInstance(StructureTemplateManager.class);
    Field repo=StructureTemplateManager.class.getDeclaredField("structureRepository");repo.setAccessible(true);
    Field sizeField=StructureTemplate.class.getDeclaredField("size");sizeField.setAccessible(true);
    Field adjusted=ShipwreckPieces.ShipwreckPiece.class.getDeclaredField("heightAdjusted");adjusted.setAccessible(true);
    Field templateName=TemplateStructurePiece.class.getDeclaredField("templateName");templateName.setAccessible(true);
    var template=new StructureTemplate();var entries=new HashMap<Identifier,Optional<StructureTemplate>>();
    for(String s:List.of("with_mast","upsidedown_full","upsidedown_fronthalf","upsidedown_backhalf","sideways_full","sideways_fronthalf","sideways_backhalf","rightsideup_full","rightsideup_fronthalf","rightsideup_backhalf","with_mast_degraded","upsidedown_full_degraded","upsidedown_fronthalf_degraded","upsidedown_backhalf_degraded","sideways_full_degraded","sideways_fronthalf_degraded","sideways_backhalf_degraded","rightsideup_full_degraded","rightsideup_fronthalf_degraded","rightsideup_backhalf_degraded")) entries.put(Identifier.withDefaultNamespace("shipwreck/"+s),Optional.of(template));
    repo.set(manager,entries);
    for(int i=0;i<256;i++)for(boolean beached:new boolean[]{false,true}) {
      int x=i*17-251,z=193-i*13;long seed=i*987654321L-123456789L;
      int sx=1+i%40,sy=1+(i*7)%40,sz=1+(i*11)%40;
      sizeField.set(template,new Vec3i(sx,sy,sz));
      var generator=new Generator(source);var random=new WorldgenRandom(new LegacyRandomSource(0));random.setLargeFeatureSeed(seed,x,z);
      var context=new Structure.GenerationContext(null,generator,source,null,(qx,qy,qz)->good,null,manager,random,seed,new ChunkPos(x,z),LevelHeightAccessor.create(-64,384),h->h==good);
      var stub=new ShipwreckStructure(settings,beached).findValidGenerationPoint(context).orElseThrow();
      var piece=(ShipwreckPieces.ShipwreckPiece)stub.getPiecesBuilder().build().pieces().get(0);
      var pos=piece.templatePosition();var box=piece.getBoundingBox();
      out.printf("S %d %s %s %d %d %d %d %d %d %d %d %s %d %d %d%n",i,beached,templateName.get(piece),piece.getRotation().ordinal(),pos.getY(),box.minX(),box.minY(),box.minZ(),box.maxX(),box.maxY(),box.maxZ(),adjusted.getBoolean(piece),generator.calls,generator.hash,random.nextInt());
    }
  }
}
