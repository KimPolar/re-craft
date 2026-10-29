import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.storage.LevelData;

/** Call Java 26.3's actual border adjustment with a controlled heightmap. */
public final class WorldSpawnBorderOracle {
    private static final class ControlledLevel extends ServerLevel {
        WorldBorder border;
        int height;
        boolean queried;
        ControlledLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public WorldBorder getWorldBorder() { return border; }
        @Override public BlockPos getHeightmapPos(Heightmap.Types type, BlockPos pos) {
            if (type != Heightmap.Types.MOTION_BLOCKING) throw new AssertionError(type);
            queried = true;
            return new BlockPos(pos.getX(), height, pos.getZ());
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (JsonElement element : cases) {
            JsonObject c = element.getAsJsonObject();
            WorldBorder.Settings settings = new WorldBorder.Settings(
                c.get("cx").getAsDouble(), c.get("cz").getAsDouble(), 0.2, 5, 5, 300,
                c.get("size").getAsDouble(), c.get("duration").getAsLong(), c.get("target").getAsDouble());
            WorldBorder border = new WorldBorder(settings);
            border.applyInitialSettings(1000);
            for (int i = 0; i < c.get("ticks").getAsInt(); i++) border.tick();
            ControlledLevel level = (ControlledLevel)unsafe.allocateInstance(ControlledLevel.class);
            level.border = border;
            level.height = c.get("height").getAsInt();
            var spawn = LevelData.RespawnData.of(Level.OVERWORLD,
                new BlockPos(c.get("x").getAsInt(), c.get("y").getAsInt(), c.get("z").getAsInt()), 30.0f, -15.0f);
            var adjusted = level.getWorldBorderAdjustedRespawnData(spawn);
            var pos = adjusted.pos();
            JsonArray result = new JsonArray();
            result.add(pos.getX()); result.add(pos.getY()); result.add(pos.getZ()); result.add(level.queried);
            result.add(border.getDistanceToBorder(pos.getX(), pos.getZ()));
            results.add(result);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(results));
    }
}
