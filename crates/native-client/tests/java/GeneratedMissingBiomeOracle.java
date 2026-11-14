import net.minecraft.SharedConstants;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.dimension.LevelStem;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.presets.WorldPresets;

/** Emits one generated chunk's unzoomed biome volume in Z, X, Y order. */
public class GeneratedMissingBiomeOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var registries = VanillaRegistries.createWorldLookup();
        var stem = WorldPresets.createNormalWorldDimensions(registries)
            .get(LevelStem.OVERWORLD).orElseThrow();
        var generator = (NoiseBasedChunkGenerator) stem.generator();
        long seed = Long.parseLong(args[0]);
        int chunkX = Integer.parseInt(args[1]);
        int chunkZ = Integer.parseInt(args[2]);
        var random = RandomState.create(registries.lookupOrThrow(Registries.NOISE),
            seed, generator.generatorSettings().value());
        var resolver = generator.getBiomeSource().createUncachedResolver(random);
        var dimension = stem.type().value();
        int minQuart = Math.floorDiv(dimension.minY(), 4);
        int quartHeight = dimension.height() / 4;
        out.println("V " + dimension.minY() + " " + dimension.height());
        for (int z = 0; z < 4; z++) {
            for (int x = 0; x < 4; x++) {
                var row = new StringBuilder("B ").append(x).append(' ').append(z);
                for (int y = 0; y < quartHeight; y++) {
                    var biome = resolver.getNoiseBiome(chunkX * 4 + x, minQuart + y, chunkZ * 4 + z);
                    row.append(' ').append(biome.unwrapKey().orElseThrow().identifier());
                }
                out.println(row);
            }
        }
    }
}
