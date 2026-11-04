import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.Difficulty;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.SpawnPlacements;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.LegacyRandomSource;

public final class ChunkCreatureRulesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class); bind.setAccessible(true);
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var item : cases) {
            var test = item.getAsJsonObject();
            int mask = test.get("mask").getAsInt(), light = test.get("light").getAsInt();
            var tags = new ArrayList<net.minecraft.tags.TagKey<net.minecraft.world.level.block.Block>>();
            if ((mask & 1) != 0) tags.add(BlockTags.ANIMALS_SPAWNABLE_ON);
            if ((mask & 2) != 0) tags.add(BlockTags.GOATS_SPAWNABLE_ON);
            if ((mask & 4) != 0) tags.add(BlockTags.RABBITS_SPAWNABLE_ON);
            bind.invoke(Blocks.STONE.builtInRegistryHolder(), tags);
            var p = test.getAsJsonArray("position"); var pos = new BlockPos(p.get(0).getAsInt(), p.get(1).getAsInt(), p.get(2).getAsInt());
            var calls = new JsonArray();
            var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class}, (obj, method, a) -> {
                return switch (method.getName()) {
                    case "getRawBrightness" -> { if (!pos.equals(a[0]) || ((Integer)a[1]) != 0) throw new AssertionError("light position"); calls.add("light"); yield light; }
                    case "getBlockState" -> { if (!pos.below().equals(a[0])) throw new AssertionError("ground position"); calls.add("ground"); yield Blocks.STONE.defaultBlockState(); }
                    case "getDifficulty" -> { calls.add("difficulty"); yield test.get("peaceful").getAsBoolean() ? Difficulty.PEACEFUL : Difficulty.NORMAL; }
                    default -> throw new AssertionError("unexpected rule call " + method);
                };
            });
            var entity = BuiltInRegistries.ENTITY_TYPE.getValue(Identifier.parse(test.get("entity").getAsString()));
            var random = new LegacyRandomSource(test.get("seed").getAsLong());
            boolean result = SpawnPlacements.checkSpawnRules(entity, world, EntitySpawnReason.CHUNK_GENERATION, pos, random);
            var row = new JsonObject(); row.addProperty("result", result); row.add("calls", calls); row.addProperty("next", random.nextInt()); output.add(row);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
