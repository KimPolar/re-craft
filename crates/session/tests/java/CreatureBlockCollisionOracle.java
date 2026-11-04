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
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.world.level.BlockCollisions;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.CollisionGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.level.border.WorldBorder;
import sun.misc.Unsafe;

public final class CreatureBlockCollisionOracle {
    static final class Region extends WorldGenRegion {
        Map<BlockPos, BlockState> blocks;
        Region() { super(null, null, null, null); }
        @Override public BlockGetter getChunkForCollisions(int x, int z) { return this; }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState()); }
        @Override public int getMinY() { return -64; }
        @Override public int getHeight() { return 384; }
        @Override public WorldBorder getWorldBorder() { throw new AssertionError("null-entity collision queried border"); }
    }
    @SuppressWarnings({"unchecked", "rawtypes"})
    static BlockState property(BlockState state, String key, String value) {
        Property prop = state.getBlock().getStateDefinition().getProperty(key);
        return state.setValue(prop, (Comparable) prop.getValue(value).orElseThrow());
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var region = (Region) ((Unsafe)field.get(null)).allocateInstance(Region.class);
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
            region.blocks = blocks;
            var min = test.getAsJsonArray("min"); var max = test.getAsJsonArray("max");
            var box = new AABB(min.get(0).getAsDouble(),min.get(1).getAsDouble(),min.get(2).getAsDouble(),max.get(0).getAsDouble(),max.get(1).getAsDouble(),max.get(2).getAsDouble());
            var collisions = new BlockCollisions<>(region, (Entity) null, box, false, (pos, shape) -> pos.immutable());
            boolean blocksClear = !collisions.hasNext();
            if (!region.noEntityCollision(null, box) || !region.noBorderCollision(null, box)) throw new AssertionError("unexpected worldgen entity/border collision");
            boolean allClear = region.noCollision(box);
            if (blocksClear != allClear) throw new AssertionError("worldgen collision differs from block collision");
            output.add(allClear);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
