import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.shapes.Shapes;

public class SpeleothemFluidFacesOracle {
    static JsonObject state(BlockState state) {
        var result = new JsonObject();
        result.addProperty("id", BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
        var properties = new JsonObject();
        state.getValues().forEach(v -> properties.addProperty(v.property().getName(), v.valueName()));
        result.add("properties", properties);
        return result;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var positions = List.of(new BlockPos(0,10,0), new BlockPos(1,10,1), new BlockPos(2,10,-3),
            new BlockPos(9,10,5), new BlockPos(-7,10,2), new BlockPos(29999999,10,-29999999));
        var neighbors = new ArrayList<BlockState>();
        for (var block : List.of(Blocks.AIR, Blocks.STONE, Blocks.STONE_SLAB, Blocks.OAK_STAIRS,
                Blocks.POINTED_DRIPSTONE, Blocks.SULFUR_SPIKE)) {
            neighbors.addAll(block.getStateDefinition().getPossibleStates());
        }
        var directions = List.of(Direction.EAST, Direction.WEST, Direction.UP, Direction.DOWN, Direction.SOUTH, Direction.NORTH);
        var rows = new JsonArray();
        for (var block : List.of(Blocks.POINTED_DRIPSTONE, Blocks.SULFUR_SPIKE)) {
            for (var spike : block.getStateDefinition().getPossibleStates()) for (var pos : positions) {
                var shape = spike.getCollisionShape(EmptyBlockGetter.INSTANCE, pos);
                for (var other : neighbors) {
                    int forward = 0, reverse = 0;
                    for (int i=0; i<directions.size(); i++) {
                        var direction = directions.get(i);
                        var otherShape = other.getCollisionShape(EmptyBlockGetter.INSTANCE, pos.relative(direction));
                        if (!Shapes.mergedFaceOccludes(shape, otherShape, direction)) forward |= 1 << i;
                        if (!Shapes.mergedFaceOccludes(otherShape, shape, direction.getOpposite())) reverse |= 1 << i;
                    }
                    var row = new JsonObject(); row.add("spike", state(spike)); row.add("other", state(other));
                    var p = new JsonArray(); p.add(pos.getX()); p.add(pos.getY()); p.add(pos.getZ()); row.add("pos", p);
                    row.addProperty("forward", forward); row.addProperty("reverse", reverse); rows.add(row);
                }
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
