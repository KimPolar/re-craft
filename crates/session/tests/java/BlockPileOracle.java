import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.RotatedPillarBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.BlockPileFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.stateproviders.RotatedBlockProvider;

public class BlockPileOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var feature = new BlockPileFeature(Holder.direct(new RotatedBlockProvider(BlockStateProvider.of(Blocks.HAY_BLOCK))));
        for (int mode = 0; mode < 4; mode++) for (long seed : new long[]{0, 1, -1, 42, 987654321}) {
            var blocks = new HashMap<BlockPos, BlockState>();
            var writes = new ArrayList<String>();
            final int worldMode = mode;
            java.util.function.Function<BlockPos, BlockState> at = pos -> {
                var state = blocks.get(pos); if (state != null) return state;
                int x = pos.getX(), y = pos.getY(), z = pos.getZ();
                if (y < 8) {
                    if (worldMode == 2 && y == 7 && Math.floorMod(x + z, 4) == 0) return Blocks.AIR.defaultBlockState();
                    if ((worldMode == 1 || worldMode == 3) && y == 7 && Math.floorMod(x * 3 + z, 3) == 0) return Blocks.DIRT_PATH.defaultBlockState();
                    return Blocks.STONE.defaultBlockState();
                }
                if (worldMode == 3 && y == 8 && Math.floorMod(x + z * 2, 5) == 0) return Blocks.STONE.defaultBlockState();
                return Blocks.AIR.defaultBlockState();
            };
            var world = (WorldGenLevel) Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(), new Class<?>[]{WorldGenLevel.class}, (proxy, method, arguments) -> {
                if (method.getName().equals("getMinY")) return -16;
                if (method.getName().equals("getBlockState")) return at.apply((BlockPos) arguments[0]);
                if (method.getName().equals("isEmptyBlock")) return at.apply((BlockPos) arguments[0]).isAir();
                if (method.getName().equals("setBlock")) {
                    var pos = ((BlockPos) arguments[0]).immutable(); var state = (BlockState) arguments[1];
                    blocks.put(pos, state);
                    writes.add(String.format("WRITE\t%d\t%d\t%d\t%s\t%d", pos.getX(), pos.getY(), pos.getZ(), state.getValue(RotatedPillarBlock.AXIS).getSerializedName(), arguments[2]));
                    return true;
                }
                throw new UnsupportedOperationException(method.toString());
            });
            var random = new WorldgenRandom(new XoroshiroRandomSource(seed));
            boolean placed = feature.place(world, null, random, new BlockPos(8, 8, 8));
            out.printf("CASE\t%d\t%d\t%s\t%d\t%d%n", mode, seed, placed, random.nextLong(), writes.size());
            for (var write : writes) out.println(write);
        }
    }
}
