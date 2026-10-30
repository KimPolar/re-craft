import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.client.model.geom.ModelPart;
import net.minecraft.client.model.object.chest.ChestModel;
import net.minecraft.client.renderer.blockentity.ChestRenderer;
import net.minecraft.core.Direction;
import net.minecraft.util.RandomSource;
import org.joml.Vector3f;

public class ChestModelOracle {
    public static void main(String[] args) throws Exception {
        var result = new JsonArray();
        for (String kind : new String[]{"single", "left", "right"}) {
        var root = (kind.equals("left") ? ChestModel.createDoubleBodyLeftLayer() : kind.equals("right") ? ChestModel.createDoubleBodyRightLayer() : ChestModel.createSingleBodyLayer()).bakeRoot();
        for (Direction facing : new Direction[]{Direction.SOUTH, Direction.NORTH, Direction.WEST, Direction.EAST}) {
        var transform = ChestRenderer.modelTransformation(facing).getMatrix();
        for (String name : new String[]{"bottom", "lid", "lock"}) {
            ModelPart part = root.getChild(name);
            var cube = part.getRandomCube(RandomSource.create(1));
            for (var polygon : cube.polygons) {
                JsonObject face = new JsonObject();
                face.addProperty("kind", kind); face.addProperty("facing", facing.getName());
                face.addProperty("part", name);
                JsonArray normal = new JsonArray();
                var n=transform.transformDirection(new Vector3f(polygon.normal()));
                normal.add(n.x); normal.add(n.y); normal.add(n.z);
                face.add("normal", normal);
                JsonArray vertices = new JsonArray();
                for (var v : polygon.vertices()) {
                    JsonArray vertex = new JsonArray();
                    var p=transform.transformPosition(new Vector3f(v.worldX() + part.x / 16, v.worldY() + part.y / 16, v.worldZ() + part.z / 16));
                    vertex.add(p.x); vertex.add(p.y); vertex.add(p.z);
                    vertex.add(v.u()); vertex.add(v.v()); vertices.add(vertex);
                }
                face.add("vertices", vertices); result.add(face);
            }
        }
        }
        }
        Files.writeString(Path.of(args[0]), result.toString());
    }
}
