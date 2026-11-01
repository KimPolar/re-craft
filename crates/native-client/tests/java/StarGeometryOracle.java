import com.google.gson.Gson;
import java.nio.file.*;
import java.util.ArrayList;
import net.minecraft.util.RandomSource;
import net.minecraft.util.Mth;
import org.joml.Matrix3f;
import org.joml.Vector3f;

// The geometry calculation in the installed 26.3 SkyRenderer.buildStars.
public class StarGeometryOracle {
    public static void main(String[] args) throws Exception {
        var random = RandomSource.createThreadLocalInstance(10842L);
        var result = new ArrayList<float[]>();
        for (int i = 0; i < 1500; i++) {
            float x = random.nextFloat() * 2f - 1f;
            float y = random.nextFloat() * 2f - 1f;
            float z = random.nextFloat() * 2f - 1f;
            float size = 0.15f + random.nextFloat() * 0.1f;
            float squared = Mth.lengthSquared(x, y, z);
            if (squared <= 0.010000001f || squared >= 1f) continue;
            var center = new Vector3f(x, y, z).normalize(100f);
            float angle = (float)(random.nextDouble() * 3.1415927410125732 * 2.0);
            var matrix = new Matrix3f().rotateTowards(new Vector3f(center).negate(), new Vector3f(0, 1, 0)).rotateZ(-angle);
            for (float[] xy : new float[][] {{size, -size}, {size, size}, {-size, size}, {-size, -size}}) {
                var v = new Vector3f(xy[0], xy[1], 0).mul(matrix).add(center);
                result.add(new float[] {v.x, v.y, v.z});
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(result));
    }
}
