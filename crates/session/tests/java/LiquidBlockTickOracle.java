import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.material.*;

public class LiquidBlockTickOracle {
    static final BlockPos START = new BlockPos(8, 10, 8);

    static BlockState cell(int id) {
        return switch (id) {
            case 0 -> Blocks.AIR.defaultBlockState();
            case 1 -> Blocks.WATER.defaultBlockState();
            case 2 -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 1);
            case 3 -> Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN, true);
            case 4 -> Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN, false);
            case 5 -> Blocks.LAVA.defaultBlockState();
            case 6 -> Blocks.OAK_SLAB.defaultBlockState().setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.WATERLOGGED, true);
            case 7 -> Blocks.LAVA.defaultBlockState().setValue(LiquidBlock.LEVEL, 1);
            default -> throw new AssertionError();
        };
    }

    static String name(BlockState state) {
        var id = BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString();
        if (state.is(Blocks.BUBBLE_COLUMN)) return id + "/" + state.getValue(BubbleColumnBlock.DRAG_DOWN);
        if (state.is(Blocks.WATER) || state.is(Blocks.LAVA)) return id + "/" + state.getValue(LiquidBlock.LEVEL);
        return id;
    }

    static class World extends ServerLevel {
        Map<BlockPos, BlockState> blocks;
        JsonArray reads, writes;
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public BlockState getBlockState(BlockPos pos) {
            reads.add(pos.getY());
            return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState());
        }
        @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int depth) {
            var changed = blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState()) != state;
            var event = new JsonArray(); event.add(pos.getY()); event.add(name(state)); event.add(changed); writes.add(event);
            if (changed) blocks.put(pos.immutable(), state);
            return changed;
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world = (World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
        var tick = LiquidBlock.class.getDeclaredMethod("tick", BlockState.class, ServerLevel.class, BlockPos.class, RandomSource.class);
        tick.setAccessible(true);
        var rows = new JsonArray();
        for (int support=0; support<6; support++) for (boolean water : new boolean[]{false,true}) for (boolean lava : new boolean[]{false,true}) {
            boolean down = support==1 || support==3, up = support==2 || support==3;
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
                BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN, down ? List.of(Blocks.STONE.builtInRegistryHolder()) : List.of(),
                BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP, up ? List.of(Blocks.STONE.builtInRegistryHolder()) : List.of()))).apply();
            var fluids = new ArrayList<Holder<Fluid>>();
            if (water) fluids.add(Fluids.WATER.builtInRegistryHolder());
            if (lava) fluids.add(Fluids.LAVA.builtInRegistryHolder());
            BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(FluidTags.BUBBLE_COLUMN_CAN_OCCUPY, fluids))).apply();
            for (int original : new int[]{1,2,5,7}) for (int live=0; live<7; live++) for (int above : new int[]{0,1,3}) {
                world.blocks = new HashMap<>(); world.reads = new JsonArray(); world.writes = new JsonArray();
                world.blocks.put(START.below(), support==4 ? cell(3) : support==5 ? cell(4) : Blocks.STONE.defaultBlockState());
                world.blocks.put(START, cell(live)); world.blocks.put(START.above(), cell(above));
                var random = new LegacyRandomSource(1234);
                var source = cell(original);
                tick.invoke(source.getBlock(), source, world, START, random);
                var row = new JsonObject();
                row.addProperty("support", support); row.addProperty("water", water); row.addProperty("lava", lava);
                row.addProperty("original", original); row.addProperty("live", live); row.addProperty("above", above);
                row.add("reads", world.reads); row.add("writes", world.writes); row.addProperty("next", random.nextInt()); rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
