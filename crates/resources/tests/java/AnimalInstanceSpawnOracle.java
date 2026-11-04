import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.PathfinderMob;
import net.minecraft.world.entity.animal.Animal;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.dimension.DimensionType;
import sun.misc.Unsafe;

public final class AnimalInstanceSpawnOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var unsafeField = Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (Unsafe) unsafeField.get(null);
        var blockPos = Entity.class.getDeclaredField("blockPosition"); blockPos.setAccessible(true);
        var classes = new HashMap<String,Class<?>>();
        for (String name : List.of("pig", "cow", "sheep", "chicken", "horse", "goat", "rabbit")) {
            var field = EntityTypes.class.getField(name.toUpperCase(Locale.ROOT));
            var type = (ParameterizedType) field.getGenericType();
            var clazz = (Class<?>) type.getActualTypeArguments()[0];
            if (clazz.getMethod("checkSpawnRules", LevelAccessor.class, EntitySpawnReason.class).getDeclaringClass() != PathfinderMob.class ||
                clazz.getMethod("getWalkTargetValue", BlockPos.class, LevelReader.class).getDeclaringClass() != Animal.class)
                throw new AssertionError("unsupported animal implementation " + name);
            classes.put(name, clazz);
        }
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var item : cases) {
            var c = item.getAsJsonObject(); var p = c.getAsJsonArray("position");
            var pos = new BlockPos(p.get(0).getAsInt(), p.get(1).getAsInt(), p.get(2).getAsInt());
            var entity = (PathfinderMob) unsafe.allocateInstance(classes.get(c.get("entity").getAsString()));
            blockPos.set(entity, pos);
            var dimension = new DimensionType(false, true, false, false, 1.0, 0, 256, 256,
                null, c.get("ambient").getAsFloat(), null, null, null, null, null, Optional.empty());
            var calls = new JsonArray();
            var world = (LevelAccessor) Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(), new Class<?>[]{LevelAccessor.class}, (obj, method, a) -> {
                return switch (method.getName()) {
                    case "getBlockState" -> { if (!pos.below().equals(a[0])) throw new AssertionError("ground position"); calls.add("ground"); yield c.get("grass").getAsBoolean() ? Blocks.GRASS_BLOCK.defaultBlockState() : Blocks.STONE.defaultBlockState(); }
                    case "getSkyDarken" -> 0;
                    case "getRawBrightness" -> { if (!pos.equals(a[0]) || ((Integer)a[1]) != 0) throw new AssertionError("light position"); calls.add("light"); yield c.get("light").getAsInt(); }
                    case "dimensionType" -> { calls.add("ambient"); yield dimension; }
                    default -> { if (method.isDefault()) yield InvocationHandler.invokeDefault(obj, method, a); throw new AssertionError("unexpected call " + method); }
                };
            });
            boolean result = entity.checkSpawnRules(world, EntitySpawnReason.CHUNK_GENERATION);
            var row = new JsonObject(); row.addProperty("result", result); row.add("calls", calls); output.add(row);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
