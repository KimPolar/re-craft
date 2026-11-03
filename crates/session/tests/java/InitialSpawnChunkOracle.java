import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Optional;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.server.level.PlayerSpawnFinder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.levelgen.Heightmap;

/** Run the installed game's actual per-chunk initial-spawn finder. */
public final class InitialSpawnChunkOracle {
    static final class ControlledLevel extends ServerLevel {
        ControlledChunk chunk;
        DimensionType type;
        ControlledLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public LevelChunk getChunk(int x, int z) { return chunk; }
        @Override public DimensionType dimensionType() { return type; }
        @Override public int getMinY() { return -4; }
        @Override public BlockState getBlockState(BlockPos pos) {
            return pos.getY() == 0 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
        }
    }
    static final class ControlledChunk extends LevelChunk {
        int target;
        JsonArray trace;
        ControlledChunk() { super((Level) null, (ChunkPos) null); }
        @Override public int getHeight(Heightmap.Types type, int x, int z) {
            if (type == Heightmap.Types.MOTION_BLOCKING) {
                JsonArray entry = new JsonArray(); entry.add(x); entry.add(z); trace.add(entry);
            }
            return x * 16 + z == target ? 0 : -5;
        }
    }
    static Object allocate(Class<?> type) throws Exception {
        Class<?> u = Class.forName("sun.misc.Unsafe");
        Field f = u.getDeclaredField("theUnsafe"); f.setAccessible(true);
        return u.getMethod("allocateInstance", Class.class).invoke(f.get(null), type);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        JsonArray output = new JsonArray();
        for (int[] chunk : new int[][]{{0,0}, {-2,3}, {19,-27}}) {
            for (int target : new int[]{-1,0,1,15,16,127,255}) {
                ControlledLevel world = (ControlledLevel) allocate(ControlledLevel.class);
                world.chunk = (ControlledChunk) allocate(ControlledChunk.class);
                world.chunk.target = target; world.chunk.trace = new JsonArray();
                world.type = new DimensionType(false, true, false, false, 1.0, -16, 32, 32, null, 0.0f, null, null, null, null, null, Optional.empty());
                BlockPos found = PlayerSpawnFinder.getSpawnPosInChunk(world, new ChunkPos(chunk[0], chunk[1]));
                JsonObject result = new JsonObject();
                result.addProperty("cx", chunk[0]); result.addProperty("cz", chunk[1]); result.addProperty("target", target);
                if (found != null) { JsonArray p = new JsonArray(); p.add(found.getX()); p.add(found.getY()); p.add(found.getZ()); result.add("pos", p); }
                else result.add("pos", JsonNull.INSTANCE);
                result.add("trace", world.chunk.trace); output.add(result);
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(output));
    }
}
