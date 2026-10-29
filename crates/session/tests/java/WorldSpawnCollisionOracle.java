import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.server.level.PlayerSpawnFinder;
import net.minecraft.world.level.CollisionGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.FenceBlock;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.phys.Vec3;

/** Run PlayerSpawnFinder's real block and liquid collision query. */
public final class WorldSpawnCollisionOracle {
    static BlockState block(int code) {
        return switch (code) {
            case 1 -> Blocks.STONE.defaultBlockState();
            case 2 -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM);
            case 3 -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP);
            case 4 -> Blocks.WATER.defaultBlockState();
            case 5 -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 7);
            case 6 -> Blocks.LAVA.defaultBlockState().setValue(LiquidBlock.LEVEL, 15);
            case 7 -> Blocks.OAK_FENCE.defaultBlockState().setValue(FenceBlock.NORTH, true).setValue(FenceBlock.SOUTH, true)
                .setValue(FenceBlock.EAST, true).setValue(FenceBlock.WEST, true);
            case 8 -> Blocks.BUBBLE_COLUMN.defaultBlockState();
            case 9 -> Blocks.KELP.defaultBlockState();
            case 10 -> Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM).setValue(SlabBlock.WATERLOGGED, true);
            default -> throw new IllegalArgumentException("block " + code);
        };
    }
    static final class World implements InvocationHandler {
        final Map<BlockPos,BlockState> blocks = new HashMap<>();
        @Override public Object invoke(Object proxy, Method method, Object[] args) throws Throwable {
            return switch (method.getName()) {
                case "getChunkForCollisions" -> proxy;
                case "getBlockState" -> blocks.getOrDefault((BlockPos)args[0], Blocks.AIR.defaultBlockState());
                case "getEntityCollisions" -> List.of();
                case "getBlockEntity" -> null;
                case "getMinY" -> -16;
                case "getHeight" -> 48;
                default -> method.isDefault() ? InvocationHandler.invokeDefault(proxy, method, args)
                    : throwUnexpected(method);
            };
        }
        static Object throwUnexpected(Method method) { throw new UnsupportedOperationException(method.toString()); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Method query = PlayerSpawnFinder.class.getDeclaredMethod("noCollisionNoLiquid", CollisionGetter.class, BlockPos.class);
        query.setAccessible(true);
        Method fixup = PlayerSpawnFinder.class.getDeclaredMethod("fixupSpawnHeight", CollisionGetter.class, BlockPos.class);
        fixup.setAccessible(true);
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray out = new JsonArray();
        for (JsonElement item : cases) {
            JsonObject c = item.getAsJsonObject();
            World world = new World();
            for (JsonElement element : c.getAsJsonArray("cells")) {
                JsonArray v = element.getAsJsonArray();
                world.blocks.put(new BlockPos(v.get(0).getAsInt(), v.get(1).getAsInt(), v.get(2).getAsInt()), block(v.get(3).getAsInt()));
            }
            CollisionGetter level = (CollisionGetter)Proxy.newProxyInstance(
                WorldSpawnCollisionOracle.class.getClassLoader(), new Class<?>[]{CollisionGetter.class}, world);
            BlockPos position = new BlockPos(c.get("x").getAsInt(), c.get("y").getAsInt(), c.get("z").getAsInt());
            if (c.has("fixup") && c.get("fixup").getAsBoolean()) {
                Vec3 result = (Vec3)fixup.invoke(null, level, position);
                JsonArray coords = new JsonArray(); coords.add(result.x); coords.add(result.y); coords.add(result.z);
                out.add(coords);
            } else {
                out.add((boolean)query.invoke(null, level, position));
            }
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(out));
    }
}
