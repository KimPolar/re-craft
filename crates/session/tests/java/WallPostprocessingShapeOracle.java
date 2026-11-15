import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.*;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.ticks.*;

/** Actual wall shape callbacks, including data-pack tags and duplicate tick requests. */
public class WallPostprocessingShapeOracle {
    static final Direction[] SIDES = {Direction.NORTH, Direction.EAST, Direction.SOUTH, Direction.WEST};

    static BlockState wall(int code, boolean up, boolean wet) {
        var state = Blocks.STONE_BRICK_WALL.defaultBlockState()
            .setValue(WallBlock.UP, up).setValue(WallBlock.WATERLOGGED, wet);
        for (var direction : SIDES) {
            int side = code % 3; code /= 3;
            state = state.setValue(WallBlock.PROPERTY_BY_DIRECTION.get(direction), WallSide.values()[side]);
        }
        return state;
    }

    static int sideCode(BlockState state) {
        int code = 0, factor = 1;
        for (var direction : SIDES) {
            code += state.getValue(WallBlock.PROPERTY_BY_DIRECTION.get(direction)).ordinal() * factor;
            factor *= 3;
        }
        return code;
    }

    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var neighbors = List.of(
            Blocks.AIR.defaultBlockState(), Blocks.STONE.defaultBlockState(),
            Blocks.WATER.defaultBlockState(), wall(0, true, false),
            Blocks.IRON_BARS.defaultBlockState(), Blocks.OAK_FENCE.defaultBlockState(),
            Blocks.OAK_FENCE_GATE.defaultBlockState().setValue(FenceGateBlock.FACING, Direction.NORTH),
            Blocks.OAK_FENCE_GATE.defaultBlockState().setValue(FenceGateBlock.FACING, Direction.EAST),
            Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM),
            Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP),
            Blocks.OAK_LEAVES.defaultBlockState(), Blocks.MELON.defaultBlockState(),
            Blocks.GLASS_PANE.defaultBlockState(), Blocks.STONE_BRICK_STAIRS.defaultBlockState(),
            Blocks.BARRIER.defaultBlockState());
        var above = List.of(neighbors.get(0), neighbors.get(1), neighbors.get(8), neighbors.get(9),
            wall(0, true, false), wall(40, false, false), neighbors.get(13));
        var pos = new BlockPos(8, 8, 8);
        out.println("BEGIN_RESULT");
        for (int mask = 0; mask < 4; mask++) {
            var walls = new ArrayList<Holder<Block>>();
            if ((mask & 1) != 0) { walls.add(Blocks.STONE_BRICK_WALL.builtInRegistryHolder()); walls.add(Blocks.STONE.builtInRegistryHolder()); }
            if ((mask & 2) != 0) walls.add(Blocks.WATER.builtInRegistryHolder());
            List<Holder<Block>> override = new ArrayList<>();
            List<Holder<Block>> shulkers = new ArrayList<>();
            if ((mask & 1) != 0) override.add(Blocks.AIR.builtInRegistryHolder());
            if ((mask & 2) != 0) shulkers.add(Blocks.STONE.builtInRegistryHolder());
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,
                Map.of(BlockTags.WALLS, walls, BlockTags.WALL_POST_OVERRIDE, override,
                    BlockTags.SHULKER_BOXES, shulkers))).apply();
            for (int code = 0; code < 81; code++) for (int up = 0; up < 2; up++)
            for (int wet = 0; wet < 2; wet++) for (var direction : Direction.values())
            for (int n = 0; n < neighbors.size(); n++) {
                if (direction == Direction.DOWN && n != 0 || direction == Direction.UP && n >= 6) continue;
                if ((code + up + wet + n + direction.ordinal()) % 4 != mask) continue;
                int a = (code / 4 + n * 2 + direction.ordinal()) % above.size();
                var state = wall(code, up != 0, wet != 0);
                var neighbor = neighbors.get(n);
                var top = above.get(a);
                var fluids = new ProtoChunkTicks<net.minecraft.world.level.material.Fluid>();
                int[] requests = {0};
                var world = (ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
                    new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, values) -> switch (method.getName()) {
                        case "getBlockState" -> {
                            if (pos.above().equals(values[0])) yield top;
                            if (pos.relative(direction).equals(values[0])) yield neighbor;
                            if (pos.equals(values[0])) yield state;
                            throw new AssertionError("unexpected read " + values[0]);
                        }
                        case "scheduleTick" -> {
                            if (!pos.equals(values[0]) || values[1] != Fluids.WATER || (int)values[2] != 5)
                                throw new AssertionError("unexpected tick " + Arrays.toString(values));
                            requests[0]++;
                            fluids.schedule(new ScheduledTick<>(Fluids.WATER, pos, 5, 0L));
                            yield null;
                        }
                        default -> throw new UnsupportedOperationException(method.toString());
                    });
                var random = new LegacyRandomSource(17);
                var first = state.updateShape(world, world, pos, direction, pos.relative(direction), neighbor, random);
                var second = state.updateShape(world, world, pos, direction, pos.relative(direction), neighbor, random);
                if (sideCode(first) != sideCode(second) || first.getValue(WallBlock.UP) != second.getValue(WallBlock.UP)
                    || first.getValue(WallBlock.WATERLOGGED) != second.getValue(WallBlock.WATERLOGGED))
                    throw new AssertionError("inconsistent wall callback");
                out.printf("W %d %d %d %s %d %d %d %d %s %d %d %d%n", code, up, wet,
                    direction.getSerializedName(), n, a, mask, sideCode(first), first.getValue(WallBlock.UP),
                    requests[0], fluids.pack(0).size(), random.nextInt());
            }
        }
        out.println("END_RESULT");
    }
}
