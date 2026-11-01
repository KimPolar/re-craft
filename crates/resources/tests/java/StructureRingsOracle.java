import com.mojang.serialization.MapCodec;
import java.lang.reflect.*;
import java.util.*;
import java.util.concurrent.CompletableFuture;
import java.util.stream.Stream;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.chunk.ChunkGeneratorStructureState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.levelgen.structure.StructureSet;
import net.minecraft.world.level.levelgen.structure.placement.ConcentricRingsStructurePlacement;

public class StructureRingsOracle {
  static class Source extends BiomeSource {
    final Holder<Biome> yes,no;final int mode;
    Source(Holder<Biome> yes,Holder<Biome> no,int mode){this.yes=yes;this.no=no;this.mode=mode;}
    protected MapCodec<? extends BiomeSource> codec(){return null;}
    protected Stream<Holder<Biome>> collectPossibleBiomes(){return Stream.of(yes,no);}
    public BiomeResolver createResolver(Climate.Sampler ignored){return (x,y,z)->mode==0?no:mode==1?yes:(((x*31+z*17)&7)==0?yes:no);}
  }
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var registries=VanillaRegistries.createWorldLookup();var biomes=registries.lookupOrThrow(Registries.BIOME);
    var yes=biomes.getOrThrow(Biomes.PLAINS);var no=biomes.getOrThrow(Biomes.DESERT);
    var settings=((NoiseBasedChunkGenerator)WorldPresets.getNormalOverworld(registries).generator()).generatorSettings().value();
    var ctor=ChunkGeneratorStructureState.class.getDeclaredConstructors()[0];ctor.setAccessible(true);
    var method=Arrays.stream(ChunkGeneratorStructureState.class.getDeclaredMethods()).filter(m->m.getName().equals("generateRingPositions")).findFirst().orElseThrow();method.setAccessible(true);
    for(int i=0;i<args.length;i+=5){int distance=Integer.parseInt(args[i]),spread=Integer.parseInt(args[i+1]),count=Integer.parseInt(args[i+2]);long seed=Long.parseLong(args[i+3]);int mode=Integer.parseInt(args[i+4]);
      var random=RandomState.create(registries.lookupOrThrow(Registries.NOISE),seed,settings);
      var source=new Source(yes,no,mode);var state=(ChunkGeneratorStructureState)ctor.newInstance(random,source,seed,ChunkPos.ZERO,seed,List.of());
      var placement=new ConcentricRingsStructurePlacement(distance,spread,count,HolderSet.direct(yes));
      var set=Holder.direct(new StructureSet(List.of(),placement));
      @SuppressWarnings("unchecked") var future=(CompletableFuture<List<ChunkPos>>)method.invoke(state,set,placement);
      out.println("S");for(var p:future.join())out.printf("P %d %d%n",p.x(),p.z());
    }
  }
}
