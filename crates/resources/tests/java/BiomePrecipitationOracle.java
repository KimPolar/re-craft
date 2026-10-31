import java.io.PrintStream;
import java.lang.reflect.Method;
import java.util.Random;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.biome.*;

public class BiomePrecipitationOracle {
  public static void main(String[] args) throws Exception {
    PrintStream output = System.out;
    SharedConstants.tryDetectVersion();
    Bootstrap.bootStrap();
    Method adjusted = Biome.class.getDeclaredMethod("getHeightAdjustedTemperature", BlockPos.class, int.class);
    adjusted.setAccessible(true);
    var random = new Random(240124);
    for (int i = 0; i < 12000; ++i) {
      int x = random.nextInt(60000001) - 30000000;
      int z = random.nextInt(60000001) - 30000000;
      int y = random.nextInt(720) - 128;
      int sea = random.nextInt(256) - 64;
      float base = switch (i % 7) {
        case 0 -> 0.15f;
        case 1 -> Math.nextDown(0.15f);
        case 2 -> Math.nextUp(0.15f);
        case 3 -> 0.0f;
        case 4 -> 0.2f;
        case 5 -> 0.8f;
        default -> -0.5f;
      };
      boolean frozen = (i & 1) != 0;
      boolean wet = i % 5 != 0;
      var biome = new Biome.BiomeBuilder().temperature(base).downfall(0.5f).hasPrecipitation(wet)
        .temperatureAdjustment(frozen ? Biome.TemperatureModifier.FROZEN : Biome.TemperatureModifier.NONE)
        .specialEffects(new BiomeSpecialEffects.Builder().waterColor(0).build())
        .generationSettings(BiomeGenerationSettings.EMPTY).build();
      var pos = new BlockPos(x, y, z);
      float temp = (float) adjusted.invoke(biome, pos, sea);
      output.printf("%d %d %d %d %s %s %d %d %s%n", x, y, z, sea, frozen, wet,
        Float.floatToRawIntBits(base), Float.floatToRawIntBits(temp), biome.getPrecipitationAt(pos, sea));
    }
  }
}
