import java.io.*;
import java.lang.reflect.*;
import java.util.Random;
import net.minecraft.client.renderer.WeatherEffectRenderer;
import net.minecraft.util.RandomSource;
import sun.misc.Unsafe;

public class WeatherColumnOracle {
  public static void main(String[] args) throws Exception {
    Field field = Unsafe.class.getDeclaredField("theUnsafe");
    field.setAccessible(true);
    var renderer = (WeatherEffectRenderer)((Unsafe)field.get(null)).allocateInstance(WeatherEffectRenderer.class);
    Class<?>[] signature = {RandomSource.class, long.class, int.class, int.class, int.class, int.class, int.class, float.class};
    Method rain = WeatherEffectRenderer.class.getDeclaredMethod("createRainColumnInstance", signature);
    Method snow = WeatherEffectRenderer.class.getDeclaredMethod("createSnowColumnInstance", signature);
    rain.setAccessible(true); snow.setAccessible(true);
    var random = new Random(741721L);
    var source = RandomSource.createThreadLocalInstance(0);
    try (var out = new PrintWriter(args[0])) {
      for (int i = 0; i < 12000; i++) {
        int x = random.nextInt(60000001) - 30000000;
        int z = random.nextInt(60000001) - 30000000;
        long time = random.nextLong();
        float partial = random.nextFloat();
        int block = random.nextInt(16), sky = random.nextInt(16);
        int light = (block << 4) | (sky << 20);
        int seed = (x*x*3121+x*45238971) ^ (z*z*418711+z*13761);
        source.setSeed(seed);
        boolean snowy = (i & 1) != 0;
        var col = (WeatherEffectRenderer.ColumnInstance)(snowy ? snow : rain)
          .invoke(renderer, source, time, x, -64, 320, z, light, partial);
        out.printf("%d %d %d %d %d %d %d %d %d %d%n", x, z, time,
          Float.floatToRawIntBits(partial), block, sky, snowy ? 1 : 0,
          Float.floatToRawIntBits(col.uOffset()), Float.floatToRawIntBits(col.vOffset()), col.lightCoords());
      }
    }
  }
}
