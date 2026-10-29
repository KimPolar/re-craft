import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.server.level.PlayerSpawnFinder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.CollisionGetter;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;

/** Invoke the real private 26.3 spawn height adjustment with controlled collisions. */
public final class WorldSpawnOracle {
    // The constructor is never invoked. allocateInstance lets the oracle feed
    // controlled data into the real static search without starting a server.
    private static final class ControlledLevel extends ServerLevel {
        ControlledChunk chunk;
        Map<Integer, BlockState> blocks;
        DimensionType type;
        int min;
        ControlledLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public LevelChunk getChunk(int x, int z) { return chunk; }
        @Override public DimensionType dimensionType() { return type; }
        @Override public int getMinY() { return min; }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos.getY(), Blocks.AIR.defaultBlockState()); }
    }
    private static final class ControlledChunk extends LevelChunk {
        int motion, surface, ocean;
        ControlledChunk() { super((Level) null, (ChunkPos) null); }
        @Override public int getHeight(Heightmap.Types type, int x, int z) {
            return switch (type) {
                case MOTION_BLOCKING -> motion;
                case WORLD_SURFACE -> surface;
                case OCEAN_FLOOR -> ocean;
                default -> throw new AssertionError(type);
            };
        }
    }
    private static Object allocate(Class<?> type) throws Exception {
        Class<?> unsafeClass = Class.forName("sun.misc.Unsafe");
        Field field = unsafeClass.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        return unsafeClass.getMethod("allocateInstance", Class.class).invoke(field.get(null), type);
    }
    private static final class ControlledCollisions implements InvocationHandler {
        final int min, max;
        final Set<Integer> blocked;
        final JsonArray trace = new JsonArray();
        ControlledCollisions(JsonObject c) {
            min = c.get("min").getAsInt();
            max = c.get("max").getAsInt();
            blocked = new HashSet<>();
            for (JsonElement y : c.getAsJsonArray("blocked")) blocked.add(y.getAsInt());
        }
        @Override public Object invoke(Object proxy, Method method, Object[] args) {
            return switch (method.getName()) {
                case "getMinY" -> min;
                case "getMaxY" -> max;
                case "noCollision" -> {
                    if (args.length != 3 || args[0] != null || !((Boolean) args[2]))
                        throw new AssertionError("unexpected collision query");
                    int y = (int) Math.floor(((AABB) args[1]).minY);
                    trace.add(y);
                    yield !blocked.contains(y);
                }
                default -> throw new UnsupportedOperationException(method.toString());
            };
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        Method fixup = PlayerSpawnFinder.class.getDeclaredMethod("fixupSpawnHeight", CollisionGetter.class, BlockPos.class);
        fixup.setAccessible(true);
        Method column = PlayerSpawnFinder.class.getDeclaredMethod("getLevelRespawnPos", ServerLevel.class, int.class, int.class);
        column.setAccessible(true);
        Constructor<PlayerSpawnFinder> constructor = PlayerSpawnFinder.class.getDeclaredConstructor(ServerLevel.class, BlockPos.class, int.class);
        constructor.setAccessible(true);
        Field count = PlayerSpawnFinder.class.getDeclaredField("candidateCount");
        Field coprime = PlayerSpawnFinder.class.getDeclaredField("coprime");
        Field offset = PlayerSpawnFinder.class.getDeclaredField("offset");
        count.setAccessible(true); coprime.setAccessible(true); offset.setAccessible(true);
        JsonObject input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
        JsonObject output = new JsonObject();
        JsonArray heights = new JsonArray();
        for (JsonElement item : input.getAsJsonArray("heights")) {
            JsonObject c = item.getAsJsonObject();
            ControlledCollisions controlled = new ControlledCollisions(c);
            CollisionGetter world = (CollisionGetter) Proxy.newProxyInstance(
                WorldSpawnOracle.class.getClassLoader(), new Class<?>[]{CollisionGetter.class}, controlled);
            BlockPos origin = new BlockPos(c.get("x").getAsInt(), c.get("y").getAsInt(), c.get("z").getAsInt());
            Vec3 result = (Vec3) fixup.invoke(null, world, origin);
            JsonObject r = new JsonObject();
            JsonArray pos = new JsonArray(); pos.add(result.x); pos.add(result.y); pos.add(result.z);
            r.add("pos", pos); r.add("trace", controlled.trace); heights.add(r);
        }
        output.add("heights", heights);
        JsonArray columns = new JsonArray();
        for (JsonElement item : input.getAsJsonArray("columns")) {
            JsonObject c = item.getAsJsonObject();
            ControlledLevel world = (ControlledLevel) allocate(ControlledLevel.class);
            world.chunk = (ControlledChunk) allocate(ControlledChunk.class);
            world.chunk.motion = c.get("motion").getAsInt();
            world.chunk.surface = c.get("surface").getAsInt();
            world.chunk.ocean = c.get("ocean").getAsInt();
            world.min = -4;
            world.type = new DimensionType(false, true, false, false, 1.0, -16, 32, 32, null, 0.0f, null, null, null, null, null, Optional.empty());
            world.blocks = new HashMap<>();
            for (JsonElement cell : c.getAsJsonArray("cells")) {
                JsonArray entry = cell.getAsJsonArray();
                BlockState block = switch (entry.get(1).getAsInt()) {
                    case 1 -> Blocks.STONE.defaultBlockState();
                    case 2 -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM);
                    case 3 -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP);
                    case 4 -> Blocks.WATER.defaultBlockState();
                    default -> throw new AssertionError("invalid block");
                };
                world.blocks.put(entry.get(0).getAsInt(), block);
            }
            BlockPos found = (BlockPos) column.invoke(null, world, 8, -9);
            if (found == null) columns.add(JsonNull.INSTANCE);
            else { JsonArray pos = new JsonArray(); pos.add(found.getX()); pos.add(found.getY()); pos.add(found.getZ()); columns.add(pos); }
        }
        output.add("columns", columns);
        JsonArray choices = new JsonArray();
        for (JsonElement item : input.getAsJsonArray("radii")) {
            PlayerSpawnFinder finder = constructor.newInstance(null, BlockPos.ZERO, item.getAsInt());
            JsonArray result = new JsonArray();
            result.add(count.getInt(finder)); result.add(coprime.getInt(finder)); result.add(offset.getInt(finder));
            choices.add(result);
        }
        output.add("choices", choices);
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
    }
}
