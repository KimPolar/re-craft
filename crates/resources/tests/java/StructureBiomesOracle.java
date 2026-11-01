import java.util.*;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.tags.TagKey;
import net.minecraft.tags.TagLoader;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.DataPackConfig;
import net.minecraft.world.level.WorldDataConfiguration;
import net.minecraft.world.level.validation.DirectoryValidator;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.dimension.LevelStem;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.levelgen.structure.Structure;

public class StructureBiomesOracle {
  static Collection<Holder<Biome>> values(HolderSet<Biome> set,Map<TagKey<Biome>,List<Holder<Biome>>> tags) {
    return set.unwrapKey().map(key->Objects.requireNonNull(tags.get(key),"missing biome tag "+key)).orElseGet(()->set.stream().toList());
  }
  public static void main(String[] args) throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var registries=VanillaRegistries.createWorldLookup();var dims=WorldPresets.createNormalWorldDimensions(registries);
    var repo=ServerPacksSource.createPackRepository(Path.of(args[0]),new DirectoryValidator(path->false));
    MinecraftServer.configurePackRepository(repo,new WorldDataConfiguration(new DataPackConfig(List.of("vanilla"),List.of()),FeatureFlags.DEFAULT_FLAGS),false,false);
    try(var manager=new MultiPackResourceManager(PackType.SERVER_DATA,repo.openAllSelected())) {
    var biomeLookup=registries.lookupOrThrow(Registries.BIOME);
    TagLoader.ElementLookup<Holder<Biome>> lookup=(id,required)->biomeLookup.get(net.minecraft.resources.ResourceKey.create(Registries.BIOME,id)).map(h->(Holder<Biome>)h);
    var tags=TagLoader.loadTagsForRegistry(manager,Registries.BIOME,lookup);
    var all=new LinkedHashMap<String,Set<Holder<Biome>>>();
    for(var key:List.of(LevelStem.OVERWORLD,LevelStem.NETHER,LevelStem.END)) {
      var biomes=dims.get(key).orElseThrow().generator().getBiomeSource().possibleBiomes();
      String name=key.identifier().toString();all.put(name,biomes);
      for(var biome:biomes)out.printf("B %s %s%n",name,biome.unwrapKey().orElseThrow().identifier());
    }
    var union=new HashSet<Holder<Biome>>();all.values().forEach(union::addAll);
    var structures=new LinkedHashMap<String,Holder<Structure>>();
    registries.lookupOrThrow(Registries.STRUCTURE_SET).listElements().forEach(holder->{
      var set=holder.value();String name=holder.key().identifier().toString();
      for(var entry:set.structures())structures.put(entry.structure().unwrapKey().orElseThrow().identifier().toString(),entry.structure());
      for(var dim:all.entrySet()) {
        boolean present=set.structures().stream().flatMap(e->values(e.structure().value().biomes(),tags).stream()).anyMatch(dim.getValue()::contains);
        out.printf("S %s %s %s%n",dim.getKey(),name,present);
      }
    });
    structures.forEach((id,holder)-> {for(var biome:union)out.printf("P %s %s %s%n",id,biome.unwrapKey().orElseThrow().identifier(),values(holder.value().biomes(),tags).contains(biome));});
    }
  }
}
