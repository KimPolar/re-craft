import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.BlockCollisions;
import net.minecraft.world.level.CollisionGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.phys.AABB;

public final class CreatureBlockCollisionOracle {
    @SuppressWarnings({"unchecked", "rawtypes"})
    static BlockState property(BlockState state, String key, String value) {
        Property prop = state.getBlock().getStateDefinition().getProperty(key);
        return state.setValue(prop, (Comparable) prop.getValue(value).orElseThrow());
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var element : input) {
            var test = element.getAsJsonObject(); var blocks = new HashMap<BlockPos, BlockState>();
            for (var cell : test.getAsJsonArray("blocks")) {
                var data = cell.getAsJsonObject(); var position = data.getAsJsonArray("position");
                var state = BuiltInRegistries.BLOCK.getValue(Identifier.parse(data.get("name").getAsString())).defaultBlockState();
                for (var e : data.getAsJsonObject("properties").entrySet()) state = property(state, e.getKey(), e.getValue().getAsString());
                blocks.put(new BlockPos(position.get(0).getAsInt(), position.get(1).getAsInt(), position.get(2).getAsInt()), state);
            }
            var level = (CollisionGetter) Proxy.newProxyInstance(CollisionGetter.class.getClassLoader(), new Class<?>[]{CollisionGetter.class}, (proxy, method, a) -> {
                return switch (method.getName()) {
                    case "getChunkForCollisions" -> proxy;
                    case "getBlockState" -> blocks.getOrDefault((BlockPos) a[0], Blocks.AIR.defaultBlockState());
                    case "getMinY" -> -64;
                    case "getHeight" -> 384;
                    default -> throw new AssertionError("unexpected world call " + method);
                };
            });
            var min = test.getAsJsonArray("min"); var max = test.getAsJsonArray("max");
            var box = new AABB(min.get(0).getAsDouble(),min.get(1).getAsDouble(),min.get(2).getAsDouble(),max.get(0).getAsDouble(),max.get(1).getAsDouble(),max.get(2).getAsDouble());
            var collisions = new BlockCollisions<>(level, (Entity) null, box, false, (pos, shape) -> pos.immutable());
            output.add(!collisions.hasNext());
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
