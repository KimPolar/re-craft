import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;

public class PlayerFireTraversalOracle {
    public static void main(String[] args) throws Exception {
        var output = new JsonArray();
        for (double[] c : new double[][]{
            {0.5,1,0.5,0,0,0},
            {0.5,1,0.5,0.1,0,0},
            {0.5,1,0.5,3,0,0},
            {0.5,1,0.5,-3,0,0},
            {0.5,1,0.5,0,3,0},
            {0.5,1,0.5,0,-3,0},
            {-0.3,2.7,-1.7,0,0,2.2},
            {1.3,1,1.3,0,0,-2.3}
        }) {
            var from = new Vec3(c[0], c[1], c[2]);
            var to = new Vec3(c[0]+c[3], c[1]+c[4], c[2]+c[5]);
            double half = (double)0.3f, height = (double)1.8f;
            var endBox = new AABB(to.x-half, to.y, to.z-half, to.x+half, to.y+height, to.z+half).deflate((double)1.0e-5f);
            var visits = new JsonArray();
            BlockGetter.forEachBlockIntersectedBetween(from, to, endBox, (pos, step) -> {
                var visit = new JsonArray(); visit.add(pos.getX()); visit.add(pos.getY()); visit.add(pos.getZ()); visit.add(step); visits.add(visit); return true;
            });
            var row = new JsonObject();
            var input = new JsonArray(); for (double value : c) input.add(value);
            row.add("input", input); row.add("visits", visits); output.add(row);
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(output));
    }
}
