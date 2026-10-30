import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.core.Direction;

public class BlockLightStatesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var requested = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var names = new ArrayList<String>();
        for (var request : requested) {
            if (request.getAsString().equals("@vanilla_slabs_stairs")) {
                for (var id : BuiltInRegistries.BLOCK.keySet()) {
                    var name = id.toString();
                    if (name.startsWith("minecraft:") && (name.endsWith("_slab") || name.endsWith("_stairs"))) names.add(name);
                }
            } else names.add(request.getAsString());
        }
        var result = new JsonArray();
        for (var name : names) {
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(name));
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var entry = new JsonObject();
                var properties = new JsonObject();
                state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                entry.addProperty("name", name);
                entry.add("properties", properties);
                entry.addProperty("opacity", state.getLightDampening());
                entry.addProperty("emission", state.getLightEmission());
                entry.addProperty("shaped", state.useShapeForLightOcclusion());
                var faces = new JsonArray();
                for (var direction : new Direction[]{Direction.EAST, Direction.WEST, Direction.UP, Direction.DOWN, Direction.SOUTH, Direction.NORTH}) {
                    int mask = 0;
                    if (state.canOcclude() && state.useShapeForLightOcclusion()) {
                        var shape = state.getFaceOcclusionShape(direction);
                        int axis = direction.getAxis().ordinal();
                        int a = (axis == 0) ? 1 : 0;
                        int b = (axis == 2) ? 1 : 2;
                        for (int i = 0; i < 2; i++) for (int j = 0; j < 2; j++) {
                            double x = 0.25 + i * 0.5, y = 0.25 + j * 0.5;
                            for (var box : shape.toAabbs()) {
                                double[] min = {box.minX,box.minY,box.minZ};
                                double[] max = {box.maxX,box.maxY,box.maxZ};
                                if (min[a] <= x && max[a] >= x && min[b] <= y && max[b] >= y) { mask |= 1 << (i * 2 + j); break; }
                            }
                        }
                    }
                    faces.add(mask);
                }
                entry.add("faces", faces);
                result.add(entry);
            }
        }
        Files.writeString(Path.of(args[1]), result.toString());
    }
}
