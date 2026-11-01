import com.google.gson.*;
import java.nio.file.*;
import org.joml.Matrix4f;
import org.joml.Vector3f;

// Mirrors the six matrices and corners in SkyRenderer.buildEndSky.
public class EndSkyOracle {
    public static void main(String[] args) throws Exception {
        float[][] positions = {{-100,-100,-100},{-100,-100,100},{100,-100,100},{100,-100,-100}};
        float[][] uv = {{0,0},{0,16},{16,16},{16,0}};
        JsonArray rows = new JsonArray();
        for (int face = 0; face < 6; face++) {
            Matrix4f matrix = new Matrix4f();
            switch (face) {
                case 1 -> matrix.rotationX((float)Math.PI / 2);
                case 2 -> matrix.rotationX(-(float)Math.PI / 2);
                case 3 -> matrix.rotationX((float)Math.PI);
                case 4 -> matrix.rotationZ((float)Math.PI / 2);
                case 5 -> matrix.rotationZ(-(float)Math.PI / 2);
            }
            for (int index : new int[]{0,1,2,0,2,3}) {
                var p = new Vector3f(positions[index]).mulPosition(matrix);
                JsonArray row = new JsonArray(); row.add(p.x); row.add(p.y); row.add(p.z); row.add(uv[index][0]); row.add(uv[index][1]); rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
    }
}
