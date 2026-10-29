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
import net.minecraft.world.level.border.WorldBorder;

/** Run the installed game's search while supplying controlled collision results. */
public class RespawnSearchOracle {
    static class World implements InvocationHandler {
        final boolean bed, bunk;
        final int success;
        boolean checkedBunk;
        int reads;
        BlockPos current;
        final JsonArray trace = new JsonArray();
        final WorldBorder border = new WorldBorder();

        World(boolean bed, boolean bunk, int success) {
            this.bed = bed;
            this.bunk = bunk;
            this.success = success;
        }

        @Override public Object invoke(Object proxy, Method method, Object[] args) {
            switch (method.getName()) {
                case "getBlockState":
                    if (bed && !checkedBunk) {
                        checkedBunk = true;
                        return (bunk ? BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:red_bed")) : Blocks.AIR).defaultBlockState();
                    }
                    current = ((BlockPos) args[0]).immutable();
                    reads++;
                    return Blocks.AIR.defaultBlockState();
                case "getBlockFloorHeight":
                    if (reads != 1 && reads != 2) throw new AssertionError("block reads: " + reads);
                    JsonArray sample = new JsonArray();
                    sample.add(current.getX()); sample.add(current.getY()); sample.add(current.getZ());
                    sample.add(reads == 2);
                    reads = 0;
                    int index = trace.size();
                    trace.add(sample);
                    return index == success ? 0.125d : Double.NEGATIVE_INFINITY;
                case "getBlockCollisions": return List.of();
                case "getWorldBorder": return border;
                default: throw new UnsupportedOperationException(method.toString());
            }
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (JsonElement item : cases) {
            JsonObject c = item.getAsJsonObject();
            boolean bed = c.get("bed").getAsBoolean();
            World state = new World(bed, c.get("bunk").getAsBoolean(), c.get("success").getAsInt());
            CollisionGetter level = (CollisionGetter) Proxy.newProxyInstance(
                RespawnSearchOracle.class.getClassLoader(), new Class<?>[]{CollisionGetter.class}, state);
            BlockPos pos = new BlockPos(32, 75, -48);
            var result = bed
                ? AbstractBedBlock.findStandUpPosition(EntityTypes.PLAYER, level, pos,
                    Direction.valueOf(c.get("facing").getAsString()), c.get("yaw").getAsFloat())
                : RespawnAnchorBlock.findStandUpPosition(EntityTypes.PLAYER, level, pos);
            JsonObject out = new JsonObject();
            out.add("trace", state.trace);
            if (result.isPresent()) {
                var p = result.get();
                JsonArray v = new JsonArray(); v.add(p.x); v.add(p.y); v.add(p.z);
                out.add("position", v);
            } else out.add("position", JsonNull.INSTANCE);
            results.add(out);
        }
        Files.writeString(Path.of(args[1]), new GsonBuilder().serializeNulls().create().toJson(results));
    }
}
