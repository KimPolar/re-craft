import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.attribute.*;
import net.minecraft.world.entity.MobCategory;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.NaturalSpawner;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.gamerules.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.core.BlockPos;
import net.minecraft.world.phys.Vec3;
import sun.misc.Unsafe;

public final class ChunkCreatureSpawnOracle {
    static boolean spawnMobs;
    static GameRules rules;
    static final class FakeLevel extends ServerLevel {
        FakeLevel() { super(null, null, null, null, null, null, false, 0, List.of(), false); }
        @Override public GameRules getGameRules() { return rules; }
    }
    static Unsafe unsafe() throws Exception {
        var field = Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        return (Unsafe) field.get(null);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        rules = new GameRules(FeatureFlags.DEFAULT_FLAGS) {
            @Override @SuppressWarnings("unchecked") public <T> T get(GameRule<T> rule) {
                if (rule == GameRules.SPAWN_MOBS) return (T) Boolean.valueOf(spawnMobs);
                throw new AssertionError("unexpected game rule " + rule);
            }
        };
        var u = unsafe();
        var level = (FakeLevel) u.allocateInstance(FakeLevel.class);
        var dimension = (DimensionType) u.allocateInstance(DimensionType.class);
        var border = new WorldBorder();
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var element : cases) {
            var test = element.getAsJsonObject();
            spawnMobs = test.get("enabled").getAsBoolean();
            var attr = EnvironmentAttributes.NATURAL_MOB_SPAWNS;
            var map = EnvironmentAttributeMap.CODEC.parse(JsonOps.INSTANCE, test.get("attributes")).getOrThrow();
            var settings = map.applyModifier(attr, attr.defaultValue());
            float probability = test.get("probability").getAsFloat();
            boolean noCollision = test.get("no_collision").getAsBoolean();
            var reader = new EnvironmentAttributeReader() {
                @SuppressWarnings("unchecked") public <T> T getDimensionValue(EnvironmentAttribute<T> key) {
                    if (key == EnvironmentAttributes.NATURAL_MOB_SPAWNS) return (T) settings;
                    if (key == EnvironmentAttributes.CREATURE_WORLD_GEN_SPAWN_PROBABILITY) return (T) Float.valueOf(probability);
                    return key.defaultValue();
                }
                public <T> T getValue(EnvironmentAttribute<T> key, Vec3 pos, SpatialAttributeInterpolator interpolator) { return getDimensionValue(key); }
            };
            var attempts = new JsonArray();
            int[] collisionCalls = {0};
            var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class}, (obj, method, a) -> {
                return switch (method.getName()) {
                    case "getLevel" -> level;
                    case "environmentAttributes" -> reader;
                    case "getHeight" -> {
                        var p = new JsonArray(); p.add((Integer) a[1]); p.add((Integer) a[2]); attempts.add(p); yield 64;
                    }
                    case "dimensionType" -> dimension;
                    case "getWorldBorder" -> border;
                    case "getBlockState" -> noCollision && ((BlockPos) a[0]).getY() == 63 ? Blocks.GRASS_BLOCK.defaultBlockState() : Blocks.AIR.defaultBlockState();
                    case "noCollision" -> { collisionCalls[0]++; yield false; }
                    default -> throw new AssertionError("unexpected world call " + method);
                };
            });
            int cx = test.get("x").getAsInt(), cz = test.get("z").getAsInt();
            var random = new WorldgenRandom(new LegacyRandomSource(0));
            random.setDecorationSeed(test.get("seed").getAsLong(), cx * 16, cz * 16);
            NaturalSpawner.spawnMobsForChunkGeneration(world, new BlockPos(cx * 16, 319, cz * 16), new ChunkPos(cx, cz), random);
            var result = new JsonObject(); result.add("attempts", attempts); result.addProperty("collision_calls", collisionCalls[0]); result.addProperty("next", random.nextInt()); output.add(result);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
