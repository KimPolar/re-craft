import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.material.*;
import net.minecraft.world.ticks.*;

/** Real growing-plant shape callbacks with protochunk tick storage. */
public class KelpEdgeOracle {
    static String describe(BlockState state) {
        return BuiltInRegistries.BLOCK.getKey(state.getBlock()) + ":"
            + (state.is(Blocks.KELP) ? state.getValue(GrowingPlantHeadBlock.AGE) : "-");
    }
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var kelp = Blocks.KELP.defaultBlockState().setValue(GrowingPlantHeadBlock.AGE, 17);
        var body = Blocks.KELP_PLANT.defaultBlockState();
        var supports = List.of(Blocks.STONE.defaultBlockState(), Blocks.WATER.defaultBlockState(),
            Blocks.AIR.defaultBlockState(), kelp, body,
            Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM),
            Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP));
        var aboveStates = List.of(Blocks.AIR.defaultBlockState(), kelp, body);
        var neighbors = List.of(Blocks.AIR.defaultBlockState(), Blocks.STONE.defaultBlockState(), kelp, body);
        var pos = new BlockPos(8, 8, 8);
        out.println("BEGIN_RESULT");
        for (int mask = 0; mask < 4; mask++) {
            var tagged = new ArrayList<Holder<Block>>();
            if ((mask & 1) != 0) tagged.add(Blocks.STONE.builtInRegistryHolder());
            if ((mask & 2) != 0) tagged.add(Blocks.KELP.builtInRegistryHolder());
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,
                Map.of(BlockTags.CANNOT_SUPPORT_KELP, tagged))).apply();
            for (int kind = 0; kind < 2; kind++) for (int algorithm = 0; algorithm < 2; algorithm++)
            for (long seed : new long[] {17L, -1L}) for (Direction direction : Direction.values())
            for (int support = 0; support < supports.size(); support++)
            for (int above = 0; above < aboveStates.size(); above++) for (int n = 0; n < neighbors.size(); n++) {
                if (direction != Direction.DOWN && (mask != 0 || support != 0 || above != 0)) continue;
                var belowState = supports.get(support);
                var aboveState = aboveStates.get(above);
                var blockTicks = new ProtoChunkTicks<Block>();
                var fluidTicks = new ProtoChunkTicks<Fluid>();
                var requests = new ArrayList<String>();
                var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
                    new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, values) -> switch (method.getName()) {
                        case "getBlockState" -> {
                            if (pos.below().equals(values[0])) yield belowState;
                            if (pos.above().equals(values[0])) yield aboveState;
                            throw new AssertionError("unexpected read " + values[0]);
                        }
                        case "scheduleTick" -> {
                            if (!pos.equals(values[0])) throw new AssertionError("unexpected tick position");
                            int delay = (int) values[2];
                            if (values[1] instanceof Block block) {
                                if (delay != 1) throw new AssertionError("wrong block delay");
                                requests.add("b");
                                blockTicks.schedule(new ScheduledTick<>(block, pos, delay, 0L));
                            } else if (values[1] == Fluids.WATER) {
                                if (delay != 5) throw new AssertionError("wrong water delay");
                                requests.add("f");
                                fluidTicks.schedule(new ScheduledTick<>(Fluids.WATER, pos, delay, 0L));
                            } else throw new AssertionError("unexpected tick " + values[1]);
                            yield null;
                        }
                        default -> throw new UnsupportedOperationException(method.toString());
                    });
                RandomSource random = algorithm == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                var state = kind == 0 ? body : kelp;
                var first = state.updateShape(world, world, pos, direction, pos.relative(direction), neighbors.get(n), random);
                var second = state.updateShape(world, world, pos, direction, pos.relative(direction), neighbors.get(n), random);
                var blocks = blockTicks.pack(0);
                var fluids = fluidTicks.pack(0);
                if (blocks.size() > 1 || fluids.size() > 1) throw new AssertionError("ticks not deduplicated");
                if (!blocks.isEmpty() && (blocks.get(0).type() != state.getBlock() || blocks.get(0).delay() != 0
                        || blocks.get(0).priority().getValue() != 0)) throw new AssertionError("wrong saved block tick");
                if (!fluids.isEmpty() && (fluids.get(0).type() != Fluids.WATER || fluids.get(0).delay() != 0
                        || fluids.get(0).priority().getValue() != 0)) throw new AssertionError("wrong saved fluid tick");
                out.printf("K %d %d %d %s %d %d %d %d %s %s %d %d %s %d%n", kind, algorithm, seed,
                    direction.getSerializedName(), support, above, n, mask, describe(first), describe(second),
                    blocks.size(), fluids.size(), requests.isEmpty() ? "-" : String.join("", requests), random.nextInt());
            }
        }
        out.println("END_RESULT");
    }
}
