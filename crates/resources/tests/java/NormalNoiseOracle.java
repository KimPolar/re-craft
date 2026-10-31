import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public class NormalNoiseOracle {
    public static void main(String[] args) throws Exception {
        var definitions = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        long[] seeds = {0, -1, 2345, Long.MIN_VALUE};
        double[][] points = {{0,0,0}, {-1.5, 64, 2.75}, {1234.5, -31.125, -789.25},
            {16777216, -16777216, 33554432}, {-30000000.5, 0.75, 123456789.125}, {0.001,0.002,0.003}};
        for (int index=0; index<definitions.size(); index++) {
            var noise = NormalNoise.DIRECT_CODEC.parse(JsonOps.INSTANCE, definitions.get(index)).getOrThrow();
            for (int mode=0; mode<2; mode++) for (long seed:seeds) {
                RandomSource random = mode == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                var sampler = noise.create(random);
                var range = noise.range();
                System.out.printf("S %d %d %d %d %d %d%n", index, mode, seed, random.nextLong(),
                    Float.floatToRawIntBits(range.min()), Float.floatToRawIntBits(range.max()));
                for (var p:points) {
                    System.out.printf("P %d %d %d %d %d%n", Double.doubleToRawLongBits(p[0]),
                        Double.doubleToRawLongBits(p[1]), Double.doubleToRawLongBits(p[2]),
                        Float.floatToRawIntBits(sampler.get(p[0],p[1],p[2])), Float.floatToRawIntBits(sampler.get(p[0],p[2])));
                }
            }
        }
    }
}
