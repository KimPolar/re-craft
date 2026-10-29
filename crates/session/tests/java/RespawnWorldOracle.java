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
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.level.CollisionGetter;
import net.minecraft.world.level.block.AbstractBedBlock;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.RespawnAnchorBlock;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BedPart;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.*;

/** Test actual 26.3 shapes and dismount searches over a synthetic chunk. */
public class RespawnWorldOracle {
    static BlockState state(int code, Direction facing) {
        return switch (code) {
            case 0 -> Blocks.AIR.defaultBlockState();
            case 1 -> Blocks.STONE.defaultBlockState();
            case 2 -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM);
            case 3 -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP);
            case 4 -> Blocks.WATER.defaultBlockState();
            case 5 -> Blocks.LAVA.defaultBlockState();
            case 6 -> BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:red_bed")).defaultBlockState()
                .setValue(AbstractBedBlock.FACING, facing).setValue(AbstractBedBlock.PART, BedPart.HEAD);
            case 7 -> BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:red_bed")).defaultBlockState()
                .setValue(AbstractBedBlock.FACING, facing).setValue(AbstractBedBlock.PART, BedPart.FOOT);
            case 8 -> Blocks.RESPAWN_ANCHOR.defaultBlockState();
            default -> throw new IllegalArgumentException("code " + code);
        };
    }
    static class World implements InvocationHandler {
        final Map<BlockPos,BlockState> blocks = new HashMap<>();
        final WorldBorder border = new WorldBorder();
        @Override public Object invoke(Object proxy, Method method, Object[] args) throws Throwable {
            switch (method.getName()) {
                case "getBlockState": return blocks.getOrDefault((BlockPos)args[0], Blocks.AIR.defaultBlockState());
                case "getBlockFloorHeight": return InvocationHandler.invokeDefault(proxy, method, args);
                case "getWorldBorder": return border;
                case "getBlockCollisions":
                    AABB box = (AABB)args[1]; VoxelShape query = Shapes.create(box);
                    List<VoxelShape> result = new ArrayList<>();
                    CollisionGetter level = (CollisionGetter)proxy;
                    for (var entry : blocks.entrySet()) {
                        BlockPos pos = entry.getKey(); BlockState s = entry.getValue();
                        VoxelShape shape = s.getCollisionShape(level, pos);
                        if (shape.isEmpty()) continue;
                        shape = shape.move(pos.getX(),pos.getY(),pos.getZ());
                        if (s.isCollisionShapeFullBlock(level,pos) ? shape.bounds().intersects(box)
                            : Shapes.joinIsNotEmpty(shape, query, BooleanOp.AND)) result.add(shape);
                    }
                    return result;
                default: throw new UnsupportedOperationException(method.toString());
            }
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (JsonElement item : cases) {
            JsonObject c = item.getAsJsonObject(); Direction facing = Direction.valueOf(c.get("facing").getAsString());
            World state = new World();
            for (JsonElement element : c.getAsJsonArray("cells")) {
                JsonArray v = element.getAsJsonArray();
                state.blocks.put(new BlockPos(v.get(0).getAsInt(),v.get(1).getAsInt(),v.get(2).getAsInt()),
                    state(v.get(3).getAsInt(), facing));
            }
            CollisionGetter level = (CollisionGetter)Proxy.newProxyInstance(
                RespawnWorldOracle.class.getClassLoader(),new Class<?>[]{CollisionGetter.class},state);
            BlockPos pos = new BlockPos(8,64,8);
            var found = c.get("bed").getAsBoolean()
                ? AbstractBedBlock.findStandUpPosition(EntityTypes.PLAYER, level, pos, facing, c.get("yaw").getAsFloat())
                : RespawnAnchorBlock.findStandUpPosition(EntityTypes.PLAYER, level, pos);
            if (found.isEmpty()) results.add(JsonNull.INSTANCE);
            else { var p=found.get(); JsonArray v=new JsonArray(); v.add(p.x);v.add(p.y);v.add(p.z);results.add(v); }
        }
        Files.writeString(Path.of(args[1]),new GsonBuilder().serializeNulls().create().toJson(results));
    }
}
