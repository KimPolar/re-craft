import java.lang.reflect.*;
import java.nio.file.Path;
import java.util.*;
import java.util.concurrent.CompletableFuture;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.*;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.tags.*;
import net.minecraft.resources.*;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.chunk.ChunkGeneratorStructureState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.levelgen.structure.StructureSet;
import net.minecraft.world.level.levelgen.structure.placement.ConcentricRingsStructurePlacement;
import net.minecraft.world.level.validation.DirectoryValidator;

public class VanillaStrongholdsOracle {
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var registries=VanillaRegistries.createWorldLookup();var biomeLookup=registries.lookupOrThrow(Registries.BIOME);
    var repo=ServerPacksSource.createPackRepository(Path.of(args[0]),new DirectoryValidator(path->false));
    MinecraftServer.configurePackRepository(repo,new WorldDataConfiguration(new DataPackConfig(List.of("vanilla"),List.of()),FeatureFlags.DEFAULT_FLAGS),false,false);
    try(var manager=new MultiPackResourceManager(PackType.SERVER_DATA,repo.openAllSelected())) {
      TagLoader.ElementLookup<Holder<Biome>> lookup=(id,required)->biomeLookup.get(ResourceKey.create(Registries.BIOME,id)).map(h->(Holder<Biome>)h);
      var tags=TagLoader.loadTagsForRegistry(manager,Registries.BIOME,lookup);
      var tag=TagKey.create(Registries.BIOME,Identifier.parse("minecraft:stronghold_biased_to"));
      var preferred=HolderSet.direct(Objects.requireNonNull(tags.get(tag)));
      var original=(ConcentricRingsStructurePlacement)registries.lookupOrThrow(Registries.STRUCTURE_SET).getOrThrow(ResourceKey.create(Registries.STRUCTURE_SET,Identifier.parse("minecraft:strongholds"))).value().placement();
      var placement=new ConcentricRingsStructurePlacement(original.distance(),original.spread(),original.count(),preferred);
      var set=Holder.direct(new StructureSet(List.of(),placement));
      var generator=(NoiseBasedChunkGenerator)WorldPresets.getNormalOverworld(registries).generator();
      var ctor=ChunkGeneratorStructureState.class.getDeclaredConstructors()[0];ctor.setAccessible(true);
      var method=Arrays.stream(ChunkGeneratorStructureState.class.getDeclaredMethods()).filter(m->m.getName().equals("generateRingPositions")).findFirst().orElseThrow();method.setAccessible(true);
      for(int i=1;i<args.length;i++) {long seed=Long.parseLong(args[i]);
        var random=RandomState.create(registries.lookupOrThrow(Registries.NOISE),seed,generator.generatorSettings().value());
        var state=(ChunkGeneratorStructureState)ctor.newInstance(random,generator.getBiomeSource(),seed,ChunkPos.ZERO,seed,List.of());
        @SuppressWarnings("unchecked") var future=(CompletableFuture<List<ChunkPos>>)method.invoke(state,set,placement);
        out.println("S "+seed);for(var p:future.join())out.printf("P %d %d%n",p.x(),p.z());
      }
    }
  }
}
