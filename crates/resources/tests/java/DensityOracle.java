import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import net.minecraft.server.Bootstrap;
import net.minecraft.SharedConstants;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunctions;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunctionCompiler;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;

public class DensityOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var expressions = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var points = new ArrayList<int[]>(Arrays.asList(new int[][] {
            {0, -128, 0}, {1, -64, -1}, {31, -63, -80}, {-7, 0, 16},
            {42, 63, -17}, {0, 319, 0}, {0, 320, 0}, {0, 500, 0},
            {Integer.MIN_VALUE, 7, 11}, {Integer.MAX_VALUE, -7, -11}
        }));
        for (int x : new int[] {-33, -9, -1, 0, 1, 7, 17}) {
            for (int y : new int[] {-67, -5, 0, 4, 31, 321}) {
                points.add(new int[] {x, y, x - y});
            }
        }
        for (int i = 0; i < expressions.size(); i++) {
            var function = DensityFunctions.DIRECT_CODEC.parse(JsonOps.INSTANCE, expressions.get(i)).getOrThrow();
            var range = function.range();
            System.out.printf("R %d %d %d%n", i, Float.floatToRawIntBits(range.min()), Float.floatToRawIntBits(range.max()));
            var sampler = new DensityFunctionCompiler(null).getSampler(function);
            for (var p : points) {
                int bits = Float.floatToRawIntBits(sampler.sampleValue(SamplerContext.EMPTY_UNCACHED, p[0], p[1], p[2]));
                System.out.printf("D %d %d %d %d %d%n", i, p[0], p[1], p[2], bits);
            }
        }
    }
}
