import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.server.Bootstrap;
import net.minecraft.SharedConstants;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunctions;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;

public class DensityOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var expressions = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        int[][] points = {{0, -128, 0}, {1, -64, -1}, {31, -63, -80}, {-7, 0, 16},
                          {42, 63, -17}, {0, 319, 0}, {0, 320, 0}, {0, 500, 0}};
        for (int i = 0; i < expressions.size(); i++) {
            var function = DensityFunctions.DIRECT_CODEC.parse(JsonOps.INSTANCE, expressions.get(i)).getOrThrow();
            var sampler = function.compileSampler(null);
            for (var p : points) {
                int bits = Float.floatToRawIntBits(sampler.sampleValue(SamplerContext.EMPTY_UNCACHED, p[0], p[1], p[2]));
                System.out.printf("%d %d %d %d %d%n", i, p[0], p[1], p[2], bits);
            }
        }
    }
}
