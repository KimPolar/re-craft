import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.SpawnPlacementTypes;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.NaturalSpawner;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.pathfinder.PathComputationType;

public final class MobPlacementOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var topMethod = NaturalSpawner.class.getDeclaredMethod("getTopNonCollidingPos", LevelReader.class, net.minecraft.world.entity.EntityType.class, int.class, int.class);
        topMethod.setAccessible(true);
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var element : cases) {
            var test = element.getAsJsonObject();
            int height = test.get("height").getAsInt(), minY = test.get("min_y").getAsInt();
            boolean ceiling = test.get("ceiling").getAsBoolean();
            var dimension = new DimensionType(false, true, ceiling, false, 1.0, 0, 256, 256,
                null, 0.0f, null, null, null, null, null, Optional.empty());
            var border = new WorldBorder(); border.setSize(test.get("border_size").getAsDouble());
            var blocks = test.getAsJsonObject("blocks");
            var world = (LevelReader) Proxy.newProxyInstance(LevelReader.class.getClassLoader(), new Class<?>[]{LevelReader.class}, (obj, method, a) -> {
                return switch (method.getName()) {
                    case "getHeight" -> height;
                    case "getMinY" -> minY;
                    case "dimensionType" -> dimension;
                    case "getWorldBorder" -> border;
                    case "getBlockState" -> {
                        var key = Integer.toString(((BlockPos) a[0]).getY());
                        yield blocks.has(key) ? BuiltInRegistries.BLOCK.getValue(Identifier.parse(blocks.get(key).getAsString())).defaultBlockState() : Blocks.AIR.defaultBlockState();
                    }
                    default -> throw new AssertionError("unexpected world call " + method);
                };
            });
            int x = test.get("x").getAsInt(), z = test.get("z").getAsInt();
            var top = (BlockPos) topMethod.invoke(null, world, EntityTypes.PIG, x, z);
            boolean ok = SpawnPlacementTypes.ON_GROUND.isSpawnPositionOk(world, top, EntityTypes.PIG);
            var result = new JsonObject();
            result.addProperty("top_y", top.getY()); result.addProperty("ok", ok);
            result.addProperty("within", border.isWithinBounds(top));
            var facts = new JsonObject();
            for (int y = -2; y <= 20; y++) {
                var pos = new BlockPos(x, y, z); var state = world.getBlockState(pos);
                var entry = new JsonObject();
                entry.addProperty("air", state.isAir());
                entry.addProperty("path", state.isPathfindable(PathComputationType.LAND));
                entry.addProperty("support", state.isValidSpawn(world, pos, EntityTypes.PIG));
                entry.addProperty("full", state.isCollisionShapeFullBlock(world, pos));
                entry.addProperty("signal", state.isSignalSource());
                entry.addProperty("fluid_empty", state.getFluidState().isEmpty());
                entry.addProperty("prevents", state.is(BlockTags.PREVENT_MOB_SPAWNING_INSIDE));
                entry.addProperty("dangerous", EntityTypes.PIG.isBlockDangerous(state));
                boolean universal = true;
                boolean support = state.isValidSpawn(world, pos, EntityTypes.PIG);
                boolean dangerous = EntityTypes.PIG.isBlockDangerous(state);
                for (var entity : BuiltInRegistries.ENTITY_TYPE) {
                    if (state.isValidSpawn(world, pos, entity) != support || entity.isBlockDangerous(state) != dangerous) {
                        universal = false; break;
                    }
                }
                entry.addProperty("universal", universal);
                facts.add(Integer.toString(y), entry);
            }
            result.add("facts", facts); output.add(result);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
