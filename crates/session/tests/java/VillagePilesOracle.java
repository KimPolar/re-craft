import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.random.WeightedList;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.BlockPileFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.stateproviders.WeightedStateProvider;

public class VillagePilesOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        BlockStateProvider provider = switch (args[0]) {
            case "snow" -> BlockStateProvider.of(Blocks.SNOW);
            case "melon" -> BlockStateProvider.of(Blocks.MELON);
            case "ice" -> new WeightedStateProvider(WeightedList.<BlockState>builder()
                .add(Blocks.BLUE_ICE.defaultBlockState(), 1).add(Blocks.PACKED_ICE.defaultBlockState(), 5));
            case "pumpkin" -> new WeightedStateProvider(WeightedList.<BlockState>builder()
                .add(Blocks.PUMPKIN.defaultBlockState(), 19).add(Blocks.JACK_O_LANTERN.defaultBlockState(), 1));
            default -> throw new IllegalArgumentException(args[0]);
        };
        var feature = new BlockPileFeature(Holder.direct(provider));
        for (long seed = 0; seed < 20; seed++) {
            var blocks = new HashMap<BlockPos, BlockState>();
            var writes = new ArrayList<String>();
            java.util.function.Function<BlockPos, BlockState> at = pos -> blocks.getOrDefault(pos,
                pos.getY() < 8 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState());
            var world = (WorldGenLevel) Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(), new Class<?>[]{WorldGenLevel.class}, (proxy, method, arguments) -> {
                if (method.getName().equals("getMinY")) return -16;
                if (method.getName().equals("getBlockState")) return at.apply((BlockPos) arguments[0]);
                if (method.getName().equals("isEmptyBlock")) return at.apply((BlockPos) arguments[0]).isAir();
                if (method.getName().equals("setBlock")) {
                    var pos = ((BlockPos) arguments[0]).immutable(); var state = (BlockState) arguments[1];
                    blocks.put(pos, state);
                    String extra = state.is(Blocks.SNOW) ? "layers=" + state.getValue(BlockStateProperties.LAYERS)
                        : state.is(Blocks.JACK_O_LANTERN) ? "facing=" + state.getValue(BlockStateProperties.HORIZONTAL_FACING).getSerializedName() : "";
                    writes.add(String.format("WRITE\t%d\t%d\t%d\t%s\t%s\t%d", pos.getX(), pos.getY(), pos.getZ(), BuiltInRegistries.BLOCK.getKey(state.getBlock()), extra, arguments[2]));
                    return true;
                }
                throw new UnsupportedOperationException(method.toString());
            });
            var random = new WorldgenRandom(new XoroshiroRandomSource(seed));
            boolean placed = feature.place(world, null, random, new BlockPos(8, 8, 8));
            out.printf("CASE\t%d\t%s\t%d%n", seed, placed, random.nextLong());
            for (var write : writes) out.println(write);
        }
    }
}
