import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.phys.shapes.VoxelShape;

public class SpeleothemShapesOracle {
    static JsonArray boxes(VoxelShape shape) {
        var result = new JsonArray();
        for (var box : shape.toAabbs()) {
            var coordinates = new JsonArray();
            for (double coordinate : new double[] {box.minX, box.minY, box.minZ, box.maxX, box.maxY, box.maxZ})
                coordinates.add(coordinate);
            result.add(coordinates);
        }
        return result;
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var records = new JsonArray();
        var positions = new BlockPos[] {
            new BlockPos(0, 0, 0), new BlockPos(1, 64, 0), new BlockPos(-1, -64, 2),
            new BlockPos(492, 100, 377), new BlockPos(-30000000, -16, 30000000),
            new BlockPos(Integer.MIN_VALUE, 0, Integer.MAX_VALUE)
        };
        for (var block : new net.minecraft.world.level.block.Block[] {Blocks.SULFUR_SPIKE, Blocks.POINTED_DRIPSTONE}) {
            if (!block.hasDynamicShape()) throw new AssertionError("expected dynamic shape");
            for (var state : block.getStateDefinition().getPossibleStates()) {
                for (var pos : positions) {
                    var record = new JsonObject();
                    record.addProperty("id", BuiltInRegistries.BLOCK.getKey(block).toString());
                    var properties = new JsonObject();
                    state.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                    record.add("properties", properties);
                    var position = new JsonArray();
                    position.add(pos.getX()); position.add(pos.getY()); position.add(pos.getZ());
                    record.add("position", position);
                    record.add("collision", boxes(state.getCollisionShape(EmptyBlockGetter.INSTANCE, pos)));
                    record.add("spawnCollision", boxes(state.getCollisionShape(EmptyBlockGetter.INSTANCE, pos,
                        net.minecraft.world.phys.shapes.CollisionContext.emptyWithFluidCollisions())));
                    record.add("outline", boxes(state.getShape(EmptyBlockGetter.INSTANCE, pos)));
                    records.add(record);
                }
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(records));
    }
}
