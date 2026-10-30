import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.SignalGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.material.FluidState;

public class RedstoneSignalOracle {
    static class World implements SignalGetter {
        Map<BlockPos, BlockState> states = new HashMap<>();
        public BlockState getBlockState(BlockPos pos) { return states.getOrDefault(pos, Blocks.AIR.defaultBlockState()); }
        public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
        public BlockEntity getBlockEntity(BlockPos pos) { return null; }
        public int getHeight() { return 384; }
        public int getMinY() { return -64; }
    }
    @SuppressWarnings({"rawtypes", "unchecked"})
    static BlockState set(BlockState state, Property property, String value) {
        return (BlockState)state.setValue(property, (Comparable) property.getValue(value).orElseThrow());
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var inputs = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var outputs = new JsonArray();
        var world = new World();
        for (var input : inputs) {
            world.states.clear();
            for (var raw : input.getAsJsonArray()) {
                var spec = raw.getAsJsonObject();
                var coords = spec.getAsJsonArray("pos");
                var pos = new BlockPos(coords.get(0).getAsInt(), coords.get(1).getAsInt(), coords.get(2).getAsInt());
                var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(spec.get("id").getAsString()));
                var state = block.defaultBlockState();
                for (var entry : spec.getAsJsonObject("properties").entrySet()) {
                    state = set(state, block.getStateDefinition().getProperty(entry.getKey()), entry.getValue().getAsString());
                }
                world.states.put(pos, state);
            }
            outputs.add(world.hasNeighborSignal(BlockPos.ZERO));
        }
        Files.writeString(Path.of(args[1]), outputs.toString());
    }
}
