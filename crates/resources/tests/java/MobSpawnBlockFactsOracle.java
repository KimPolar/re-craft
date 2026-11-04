import com.google.gson.*;
import java.lang.reflect.Proxy;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.pathfinder.PathComputationType;
import net.minecraft.world.phys.shapes.Shapes;

public final class MobSpawnBlockFactsOracle {
    @SuppressWarnings({"rawtypes", "unchecked"})
    static String value(BlockState state, Property property) {
        return property.getName(state.getValue(property));
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var names = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        var pos = new BlockPos(0, 8, 0);
        for (var name : names) {
            var id = Identifier.parse(name.getAsString());
            if (!BuiltInRegistries.BLOCK.containsKey(id)) throw new AssertionError(id);
            for (var state : BuiltInRegistries.BLOCK.getValue(id).getStateDefinition().getPossibleStates()) {
                var entry = new JsonObject();
                entry.addProperty("name", id.toString());
                var properties = new JsonObject();
                for (var property : state.getProperties()) properties.addProperty(property.getName(), value(state, property));
                entry.add("properties", properties);
                boolean[] first = null;
                boolean universal = true, contextIndependent = true;
                for (var neighbor : new BlockState[]{Blocks.AIR.defaultBlockState(), Blocks.STONE.defaultBlockState()}) {
                    var world = (LevelReader) Proxy.newProxyInstance(LevelReader.class.getClassLoader(), new Class<?>[]{LevelReader.class}, (obj, method, a) -> {
                        return switch (method.getName()) {
                            case "getBlockState" -> pos.equals(a[0]) ? state : neighbor;
                            case "getFluidState" -> (pos.equals(a[0]) ? state : neighbor).getFluidState();
                            case "getMinY" -> 0;
                            case "getHeight" -> 256;
                            default -> throw new AssertionError("unexpected world call " + method);
                        };
                    });
                    boolean support = state.isValidSpawn(world, pos, EntityTypes.PIG);
                    boolean dangerous = EntityTypes.PIG.isBlockDangerous(state);
                    if (first == null) {
                        var shape = state.getCollisionShape(world, pos);
                        entry.addProperty("canonical_full", shape == Shapes.block());
                        entry.addProperty("large", state.hasLargeCollisionShape());
                        var boxes = new JsonArray();
                        for (var box : shape.toAabbs()) {
                            var values = new JsonArray();
                            for (double value : new double[]{box.minX, box.minY, box.minZ, box.maxX, box.maxY, box.maxZ}) values.add(value);
                            boxes.add(values);
                        }
                        entry.add("collision_boxes", boxes);
                    }
                    boolean[] facts = {state.isAir(), state.isPathfindable(PathComputationType.LAND), support,
                        state.isCollisionShapeFullBlock(world, pos), state.isSignalSource(), state.getFluidState().isEmpty(), dangerous};
                    for (var entity : BuiltInRegistries.ENTITY_TYPE) {
                        if (state.isValidSpawn(world, pos, entity) != support || entity.isBlockDangerous(state) != dangerous) universal = false;
                    }
                    if (first == null) first = facts;
                    else if (!java.util.Arrays.equals(first, facts)) contextIndependent = false;
                }
                var facts = new JsonArray();
                for (boolean fact : first) facts.add(fact);
                entry.add("facts", facts);
                entry.addProperty("universal", universal);
                entry.addProperty("context_independent", contextIndependent);
                output.add(entry);
            }
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
