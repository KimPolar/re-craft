import java.lang.reflect.Method;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.BubbleColumnBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.Fluids;

/** Confirm that the actual 26.3 server skips stale fluid ticks on a column. */
public class BubbleStaleFluidTickOracle {
    static class World extends ServerLevel {
        BlockState block;
        int reads;

        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public BlockState getBlockState(BlockPos pos) { reads++; return block; }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        Method tick = ServerLevel.class.getDeclaredMethod("tickFluid", BlockPos.class, Fluid.class);
        tick.setAccessible(true);
        for (boolean drag : new boolean[] {false, true}) {
            world.block = Blocks.BUBBLE_COLUMN.defaultBlockState()
                    .setValue(BubbleColumnBlock.DRAG_DOWN, drag);
            var state = world.block.getFluidState();
            if (!state.is(Fluids.WATER)) throw new AssertionError("column is not source water");
            for (var fluid : new Fluid[] {Fluids.FLOWING_WATER, Fluids.LAVA, Fluids.FLOWING_LAVA}) {
                if (state.is(fluid)) throw new AssertionError("fluid was not stale");
                world.reads = 0;
                tick.invoke(world, new BlockPos(8, 5, 8), fluid);
                if (world.reads != 1) throw new AssertionError("stale tick ran its callback");
            }
        }
    }
}
