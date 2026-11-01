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
            var id = Identifier.parse(name);
            if (!BuiltInRegistries.BLOCK.containsKey(id)) throw new IllegalArgumentException("unknown block: " + name);
            var block = BuiltInRegistries.BLOCK.getValue(id);
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var entry = new JsonObject();
                var properties = new JsonObject();
                state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                entry.addProperty("name", name);
                entry.add("properties", properties);
                entry.addProperty("opacity", state.getLightDampening());
                entry.addProperty("emission", state.getLightEmission());
                entry.addProperty("shaped", state.useShapeForLightOcclusion());
                if (name.equals("minecraft:frosted_ice")) {
                    entry.addProperty("friction", block.getFriction());
                    var getter = net.minecraft.world.level.EmptyBlockGetter.INSTANCE;
                    var pos = net.minecraft.core.BlockPos.ZERO;
                    var sturdy = new JsonArray();
                    for (var direction : Direction.values()) sturdy.add(state.isFaceSturdy(getter, pos, direction));
                    entry.add("sturdy", sturdy);
                    for (var shape : new Object[][] {{"collision", state.getCollisionShape(getter, pos)}, {"outline", state.getShape(getter, pos)}}) {
                        var boxes = ((net.minecraft.world.phys.shapes.VoxelShape)shape[1]).toAabbs();
                        if (boxes.size() != 1) throw new AssertionError("unexpected ice shape");
                        var box = boxes.get(0); var points = new JsonArray();
                        for (double[] point : new double[][] {{box.minX,box.minY,box.minZ},{box.maxX,box.maxY,box.maxZ}}) {
                            var values = new JsonArray(); for (double value : point) values.add(value); points.add(values);
                        }
                        entry.add((String)shape[0], points);
                    }
                }
                var faces = new JsonArray();
                for (var direction : new Direction[]{Direction.EAST, Direction.WEST, Direction.UP, Direction.DOWN, Direction.SOUTH, Direction.NORTH}) {
                    int[] mask = new int[16];
                    if (state.canOcclude() && state.useShapeForLightOcclusion()) {
                        var shape = state.getFaceOcclusionShape(direction);
                        int axis = direction.getAxis().ordinal();
                        int a = (axis == 0) ? 1 : 0;
                        int b = (axis == 2) ? 1 : 2;
                        for (var box : shape.toAabbs()) {
                            double[] min = {box.minX,box.minY,box.minZ};
                            double[] max = {box.maxX,box.maxY,box.maxZ};
                            int a0 = (int)Math.round(min[a] * 16), a1 = (int)Math.round(max[a] * 16);
                            int b0 = (int)Math.round(min[b] * 16), b1 = (int)Math.round(max[b] * 16);
                            if (a0 != min[a] * 16 || a1 != max[a] * 16 || b0 != min[b] * 16 || b1 != max[b] * 16)
                                throw new AssertionError("non-grid face for " + state);
                            for (int i = a0; i < a1; i++) for (int j = b0; j < b1; j++) mask[i] |= 1 << j;
                        }
                    }
                    var rows = new JsonArray();
                    for (int row : mask) rows.add(row);
                    faces.add(rows);
                }
                entry.add("faces", faces);
                result.add(entry);
            }
        }
        Files.writeString(Path.of(args[1]), result.toString());
    }
}
