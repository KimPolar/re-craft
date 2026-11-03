import net.minecraft.SharedConstants;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.dimension.LevelStem;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.presets.WorldPresets;

/** Invoke the installed Java generator's real spawn-origin method. */
public final class NoiseSpawnOriginOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var registries = VanillaRegistries.createWorldLookup();
        var dimensions = WorldPresets.createNormalWorldDimensions(registries);
        var kinds = java.util.List.of(LevelStem.OVERWORLD, LevelStem.NETHER, LevelStem.END);
        long[] seeds = {0, 42, -1, 12345, Long.MIN_VALUE, Long.MAX_VALUE};
        for (int d = 0; d < kinds.size(); d++) {
            var generator = (NoiseBasedChunkGenerator) dimensions.get(kinds.get(d)).orElseThrow().generator();
            for (int[] bounds : new int[][]{{-64,384},{0,256},{80,64},{-128,16}}) {
                var level = net.minecraft.world.level.LevelHeightAccessor.create(bounds[0], bounds[1]);
                out.printf("H %d %d %d %d%n", d, bounds[0], bounds[1], generator.getSpawnHeight(level));
            }
            for (long seed : seeds) {
                var random = RandomState.create(registries.lookupOrThrow(Registries.NOISE), seed, generator.generatorSettings().value());
                var origin = generator.getOrigin(random);
                out.printf("P %d %d %d %d%n", d, seed, origin.x(), origin.z());
                if (d == 0) {
                    var parameters = new java.util.HashMap<>(generator.generatorSettings().value().spawnTarget().get(0).parameters());
                    parameters.replaceAll((key, value) -> net.minecraft.world.level.biome.Climate.Parameter.span(-0.5f, 0.5f));
                    var target = java.util.List.of(new net.minecraft.world.level.levelgen.SpawnTargetPoint(parameters));
                    var context = net.minecraft.world.level.levelgen.densityfunction.SamplerContext.builder().enableCaches().build();
                    var custom = net.minecraft.world.level.levelgen.NoiseSpawnFinder.findSpawnPosition(target, random.samplersWithContext(context));
                    out.printf("C %d %d %d%n", seed, Math.floorDiv(custom.getX(), 16), Math.floorDiv(custom.getZ(), 16));
                }
            }
        }
    }
}
