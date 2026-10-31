import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.synth.PerlinNoise;
import net.minecraft.world.level.levelgen.synth.NoiseStack;

public class PerlinNoiseOracle {
    public static void main(String[] args) {
        long[] seeds = {0, 1, -1, 2345, -987654321, Long.MIN_VALUE, Long.MAX_VALUE};
        double[] edges = {0.0, -0.0, -1.0, 0.125, -0.125, 16777215.0, 16777216.0,
                          -16777216.0, 33554432.0, -33554432.0, 30000000000.75, -30000000000.5,
                          Math.nextDown(16777216.0), Math.nextUp(16777216.0),
                          Math.nextDown(-16777216.0), Math.nextUp(-16777216.0)};
        for (int mode = 0; mode < 2; mode++) {
            for (long seed : seeds) {
                RandomSource random = mode == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                PerlinNoise noise = new PerlinNoise(random);
                System.out.printf("S %d %d %d%n", mode, seed, random.nextLong());
                RandomSource stackRandom = mode == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                var builder = NoiseStack.builder();
                builder.add(new PerlinNoise(stackRandom), 1.0, 1.0f);
                builder.add(new PerlinNoise(stackRandom), 0.25, -0.2f);
                builder.add(new PerlinNoise(stackRandom), 1.0181268882175227, 0.37f);
                builder.add(new PerlinNoise(stackRandom), 2.0, 0.0f);
                NoiseStack stack = builder.build();
                for (int i = -64; i < 64; i++) {
                    double x = i * 13.0 / 7.0;
                    double y = i * 19.0 / 11.0;
                    double z = i * -37.0 / 5.0;
                    print(noise, stack, x, y, z);
                }
                for (int i = 0; i < edges.length; i++) {
                    print(noise, stack, edges[i], edges[(i + 3) % edges.length], edges[(i + 7) % edges.length]);
                }
            }
        }
    }
    private static void print(PerlinNoise noise, NoiseStack stack, double x, double y, double z) {
        System.out.printf("P %d %d %d %d %d %d %d%n", Double.doubleToRawLongBits(x), Double.doubleToRawLongBits(y),
                Double.doubleToRawLongBits(z), Float.floatToRawIntBits(noise.get(x, y, z)), Float.floatToRawIntBits(noise.get(x, z)),
                Float.floatToRawIntBits(stack.get(x, y, z)), Float.floatToRawIntBits(stack.get(x, z)));
    }
}
