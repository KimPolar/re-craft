import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.vehicle.DismountHelper;
import net.minecraft.world.level.CollisionGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.TrapDoorBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.Shapes;

/** Use the game's floor-height and dismount implementation with controlled collisions. */
public class RespawnDismountOracle {
    static BlockState state(String name) {
        String id = name;
        if (name.equals("slab_bottom") || name.equals("slab_top")) id = "stone_slab";
        if (name.equals("trapdoor_open") || name.equals("trapdoor_closed")) id = "oak_trapdoor";
        BlockState result = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:" + id)).defaultBlockState();
        if (name.equals("slab_top")) result = result.setValue(SlabBlock.TYPE, SlabType.TOP);
        if (name.equals("trapdoor_open")) result = result.setValue(TrapDoorBlock.OPEN, true);
        return result;
    }

    static JsonObject properties(CollisionGetter level, BlockPos pos) {
        BlockState block = level.getBlockState(pos);
        var shape = DismountHelper.nonClimbableShape(level, pos);
        JsonObject result = new JsonObject();
        result.addProperty("danger", EntityTypes.PLAYER.isBlockDangerous(block));
        result.addProperty("invalid", block.is(BlockTags.INVALID_SPAWN_INSIDE));
        if (shape.isEmpty()) result.add("top", JsonNull.INSTANCE);
        else result.addProperty("top", shape.max(Direction.Axis.Y));
        return result;
    }

    static class World implements InvocationHandler {
        final BlockState current, below, above;
        final boolean collision;
        final WorldBorder border = new WorldBorder();
        AABB checkedBounds;
        World(JsonObject c) {
            current = state(c.get("current").getAsString());
            below = state(c.get("below").getAsString());
            above = state(c.get("above").getAsString());
            collision = c.get("collision").getAsBoolean();
            if (c.get("outside").getAsBoolean()) { border.setCenter(1000.0, 1000.0); border.setSize(10.0); }
        }
        @Override public Object invoke(Object proxy, Method method, Object[] args) throws Throwable {
            switch (method.getName()) {
                case "getBlockState":
                    BlockPos pos = (BlockPos) args[0];
                    if (pos.equals(new BlockPos(0, 64, 0))) return current;
                    if (pos.equals(new BlockPos(0, 63, 0))) return below;
                    if (pos.equals(new BlockPos(0, 65, 0))) return above;
                    return Blocks.AIR.defaultBlockState();
                case "getBlockFloorHeight": return InvocationHandler.invokeDefault(proxy, method, args);
                case "getBlockCollisions":
                    checkedBounds = (AABB) args[1];
                    return collision ? List.of(Shapes.block()) : List.of();
                case "getWorldBorder": return border;
                default: throw new UnsupportedOperationException(method.toString());
            }
        }
    }

    static JsonArray vector(double... values) {
        JsonArray out = new JsonArray();
        for (double value : values) out.add(value);
        return out;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (JsonElement item : cases) {
            JsonObject c = item.getAsJsonObject(); World state = new World(c);
            CollisionGetter level = (CollisionGetter) Proxy.newProxyInstance(
                RespawnDismountOracle.class.getClassLoader(), new Class<?>[]{CollisionGetter.class}, state);
            BlockPos pos = new BlockPos(0, 64, 0);
            JsonObject out = new JsonObject();
            out.add("current", properties(level, pos));
            out.add("below", properties(level, pos.below()));
            out.add("above", properties(level, pos.above()));
            var result = DismountHelper.findSafeDismountLocation(EntityTypes.PLAYER, level, pos, c.get("avoid").getAsBoolean());
            if (result == null) out.add("position", JsonNull.INSTANCE);
            else out.add("position", vector(result.x, result.y, result.z));
            AABB b = state.checkedBounds;
            if (b == null) out.add("bounds", JsonNull.INSTANCE);
            else out.add("bounds", vector(b.minX,b.minY,b.minZ,b.maxX,b.maxY,b.maxZ));
            results.add(out);
        }
        Files.writeString(Path.of(args[1]), new GsonBuilder().serializeNulls().create().toJson(results));
    }
}
