import java.io.*;
import java.nio.file.*;
import java.util.Random;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeSpecialEffects.GrassColorModifier;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.synth.SimplexNoise;

public class BiomeGrassOracle {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        try (var output = Files.newBufferedWriter(Path.of(args[0]))) {
            for (long seed : new long[]{2345L, 1234L, -1L}) {
                for (boolean zero : new boolean[]{true, false}) {
                    var simplex = new SimplexNoise(new WorldgenRandom(new LegacyRandomSource(seed)), zero);
                    var random = new Random(87654321L);
                    for (int i = 0; i < 12000; i++) {
                        int x = i < 256 ? i - 128 : random.nextInt(60000001) - 30000000;
                        int z = i < 256 ? (i % 17) - 8 : random.nextInt(60000001) - 30000000;
                        float noise = simplex.get(x * 0.0225, z * 0.0225);
                        int grass = GrassColorModifier.SWAMP.modifyColor(x, z, 0x123456);
                        output.write(seed + "," + zero + "," + x + "," + z + "," + Float.floatToIntBits(noise) + "," + grass + "\n");
                    }
                }
            }
        }
    }
}
