import com.google.gson.Gson;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import org.joml.Matrix4f;
import org.joml.Quaternionf;
import org.joml.Vector3f;

// Reproduce the 26.3 SkyRenderer celestial pose with its JOML operations.
public class SunOracle {
    public static void main(String[] args) throws Exception {
        var output = new ArrayList<float[]>();
        for (float size : new float[] {30, 20}) for (float angle : new float[] {0, 45, 90, 180, 270, 359.5f, -90, 450}) {
            var matrix = new Matrix4f()
                    .rotate(new Quaternionf().rotationY(-90.0f * 0.017453292f))
                    .rotate(new Quaternionf().rotationX(angle * 0.017453292f))
                    .translate(0, 100, 0)
                    .scale(size, 1, size);
            for (float[] vertex : new float[][] {{-1, 0, -1}, {1, 0, -1}, {1, 0, 1}, {-1, 0, 1}}) {
                var point = matrix.transformPosition(new Vector3f(vertex[0], vertex[1], vertex[2]));
                output.add(new float[] {point.x, point.y, point.z});
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(output));
    }
}
