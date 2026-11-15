import com.google.gson.JsonParser;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.*;
import net.minecraft.server.*;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.tags.TagLoader;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.validation.DirectoryValidator;

/** Compare the mansion's necessary column check with Java's real context. */
public class MansionBiomeColumnOracle {
    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var biomes = lookup.lookupOrThrow(Registries.BIOME);
        var structure = lookup.lookupOrThrow(Registries.STRUCTURE).getOrThrow(
                ResourceKey.create(Registries.STRUCTURE, Identifier.withDefaultNamespace("mansion"))).value();
        var generator = (NoiseBasedChunkGenerator) WorldPresets.getNormalOverworld(lookup).generator();
        var repo = ServerPacksSource.createPackRepository(Path.of(args[0]), new DirectoryValidator(path -> false));
        MinecraftServer.configurePackRepository(repo,
                new WorldDataConfiguration(new DataPackConfig(List.of("vanilla"), List.of()), FeatureFlags.DEFAULT_FLAGS), false, false);
        try (var manager = new MultiPackResourceManager(PackType.SERVER_DATA, repo.openAllSelected())) {
            TagLoader.ElementLookup<Holder<Biome>> elements = (id, required) ->
                    biomes.get(ResourceKey.create(Registries.BIOME, id)).map(holder -> (Holder<Biome>) holder);
            var tags = TagLoader.loadTagsForRegistry(manager, Registries.BIOME, elements);
            var vanilla = structure.biomes().unwrapKey().map(key -> Set.copyOf(Objects.requireNonNull(tags.get(key))))
                    .orElseGet(() -> Set.copyOf(structure.biomes().stream().toList()));
            var all = Set.copyOf(biomes.listElements().toList());
            var states = new HashMap<Long, RandomState>();
            var cases = JsonParser.parseString(Files.readString(Path.of(args[1]))).getAsJsonArray();
            int index = 0;
            for (var value : cases) {
                var item = value.getAsJsonObject();
                long seed = item.get("seed").getAsLong();
                int x = item.get("x").getAsInt(), z = item.get("z").getAsInt();
                var allowed = item.get("mode").getAsString().equals("all") ? all : vanilla;
                var random = states.computeIfAbsent(seed, key -> RandomState.create(
                        lookup.lookupOrThrow(Registries.NOISE), key, generator.generatorSettings().value()));
                var climate = random.createClimateSampler(SamplerContext.builder().enableCaches().build());
                var context = new Structure.GenerationContext(null, generator, generator.getBiomeSource(), climate,
                        random, null, seed, new ChunkPos(x, z), LevelHeightAccessor.create(-64, 384), allowed::contains);
                boolean possible = context.couldValidBiomeExistInTerrainColumn(x * 16 + 7, z * 16 + 7);
                if (!possible && structure.findValidGenerationPoint(context).isPresent())
                    throw new AssertionError("mansion generated despite an impossible column");
                output.printf("COLUMN %d %s%n", index++, possible);
            }
        }
    }
}
