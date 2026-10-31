import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public class LegacyNetherNoiseOracle {
    public static void main(String[] args) throws Exception {
        var definitions = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        long[] seeds = {0, -1, 2345, Long.MIN_VALUE, Long.MAX_VALUE};
        double[][] points = {{0,0,0}, {-1.5,64,2.75}, {1234.5,-31.125,-789.25},
            {16777216,-16777216,33554432}, {-30000000.5,0.75,123456789.125}, {0.001,0.002,0.003}};
        for (int i=0;i<definitions.size();i++) {
            var parameters=NormalNoise.DIRECT_CODEC.parse(JsonOps.INSTANCE,definitions.get(i)).getOrThrow();
            for (long seed:seeds) {
                var random = new LegacyRandomSource(seed);
                try {
                    var noise = parameters.createForLegacyNetherBiome(random);
                    var range=parameters.range();
                    System.out.printf("S %d %d %d %d %d%n",i,seed,random.nextLong(),Float.floatToRawIntBits(range.min()),Float.floatToRawIntBits(range.max()));
                    for (var p:points) System.out.printf("P %d %d %d %d %d%n",Double.doubleToRawLongBits(p[0]),Double.doubleToRawLongBits(p[1]),Double.doubleToRawLongBits(p[2]),Float.floatToRawIntBits(noise.get(p[0],p[1],p[2])),Float.floatToRawIntBits(noise.get(p[0],p[2])));
                } catch (IllegalArgumentException | IllegalStateException e) {
                    System.out.printf("E %d %d %d%n",i,seed,random.nextLong());
                }
            }
        }
    }
}
