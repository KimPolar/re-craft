import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunctions;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.synth.Noise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public class WorldDensityNoiseOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var fixtures = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        long[] seeds = {0, -1, 2345, Long.MIN_VALUE, Long.MAX_VALUE};
        int[][] points = {{0,0,0},{1,1,1},{-1,-1,-1},{31,63,-80},{-17,-64,49},
            {12345,-20,-9832},{30000000,100,-30000000},{Integer.MIN_VALUE,12,7},
            {Integer.MAX_VALUE,-31,-17},{0,319,0},{0,320,0},{7,13,19}};
        for (int i=0;i<fixtures.size();i++) {
            var fixture=fixtures.get(i).getAsJsonObject();
            String id=fixture.get("id").getAsString();
            var function=DensityFunctions.DIRECT_CODEC.parse(JsonOps.INSTANCE,fixture.get("java_expression")).getOrThrow();
            for (int mode=0;mode<2;mode++) for (long seed:seeds) {
                RandomSource root=mode==0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                var factory=root.forkPositional();
                var ctx=new DensityFunction.CompileContext() {
                    public Noise createNoiseSampler(Holder<NormalNoise> noise) {
                        if (id.equals("minecraft:nether/temperature")) return noise.value().createForLegacyNetherBiome(new LegacyRandomSource(seed));
                        if (id.equals("minecraft:nether/vegetation")) return noise.value().createForLegacyNetherBiome(new LegacyRandomSource(seed + 1));
                        return noise.value().create(factory.fromHashOf(Identifier.parse(id)));
                    }
                    public RandomSource createRandom(Identifier name) { return factory.fromHashOf(name); }
                    public RandomSource createEndIslandRandom() { return new LegacyRandomSource(seed); }
                };
                var sampler=function.compileSampler(ctx);
                var range=function.range();
                System.out.printf("S %d %d %d %d %d%n",i,mode,seed,Float.floatToRawIntBits(range.min()),Float.floatToRawIntBits(range.max()));
                for (var p:points) {
                    System.out.printf("P %d %d %d %d%n",p[0],p[1],p[2],Float.floatToRawIntBits(sampler.sampleValue(SamplerContext.EMPTY_UNCACHED,p[0],p[1],p[2])));
                }
            }
        }
    }
}
