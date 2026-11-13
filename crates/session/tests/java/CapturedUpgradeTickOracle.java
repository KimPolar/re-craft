import java.io.PrintStream;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.UpgradeData;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.ticks.LevelChunkTicks;
import net.minecraft.world.ticks.LevelTicks;
import net.minecraft.world.ticks.SavedTick;
import net.minecraft.world.ticks.TickPriority;

public class CapturedUpgradeTickOracle {
    static LevelTicks<Block> blocks;
    static LevelTicks<Fluid> fluids;
    static long counter;
    static class FakeLevel extends ServerLevel {
        FakeLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public BlockState getBlockState(BlockPos p) {
            if (p.getX() == 9) return Blocks.RED_SAND.defaultBlockState();
            if (p.getX() == 11) return Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 1);
            return Blocks.SAND.defaultBlockState();
        }
        @Override public net.minecraft.world.level.material.FluidState getFluidState(BlockPos p) {
            return getBlockState(p).getFluidState();
        }
        @Override public LevelTicks<Block> getBlockTicks() { return blocks; }
        @Override public LevelTicks<Fluid> getFluidTicks() { return fluids; }
        @Override public long getGameTime() { return 100; }
        @Override public long nextSubTickCount() { return counter++; }
    }
    static BlockPos p(int x) { return new BlockPos(x, 8, 8); }
    static <T> SavedTick<T> tick(T type, int x, int delay, int priority) {
        return new SavedTick<>(type, p(x), delay, TickPriority.byValue(priority));
    }
    public static void main(String[] args) throws Exception {
        PrintStream out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var level = (FakeLevel) unsafe.allocateInstance(FakeLevel.class);
        var initial = new LevelChunkTicks<Block>(List.of(tick(Blocks.SAND, 8, 0, 0)));
        var next = new LevelChunkTicks<Block>(List.of());
        var liquid = new LevelChunkTicks<Fluid>(List.of());
        blocks = new LevelTicks<>(p -> true);
        fluids = new LevelTicks<>(p -> true);
        blocks.addContainer(new ChunkPos(0, 0), initial);
        blocks.addContainer(new ChunkPos(1, 0), next);
        fluids.addContainer(new ChunkPos(0, 0), liquid);
        fluids.addContainer(new ChunkPos(1, 0), new LevelChunkTicks<>(List.of()));
        counter = 2;
        var block = UpgradeData.class.getDeclaredMethod("lambda$upgrade$0", Level.class, SavedTick.class);
        var fluid = UpgradeData.class.getDeclaredMethod("lambda$upgrade$1", Level.class, SavedTick.class);
        block.setAccessible(true);
        fluid.setAccessible(true);
        for (var value : List.of(tick(Blocks.AIR, 9, 5, -2), tick(Blocks.SAND, 8, 7, 1), tick(Blocks.STONE, 16, 3, 2)))
            block.invoke(null, level, value);
        for (var value : List.of(tick(Fluids.EMPTY, 9, 4, 0), tick(Fluids.EMPTY, 11, 6, 1), tick(Fluids.WATER, 8, 8, -1), tick(Fluids.WATER, 8, 9, 2)))
            fluid.invoke(null, level, value);
        out.println("BEGIN_RESULT");
        for (var value : initial.pack(100)) out.printf("block0 %s %d %d %d %d %d%n", BuiltInRegistries.BLOCK.getKey(value.type()), value.pos().getX(), value.pos().getY(), value.pos().getZ(), value.delay(), value.priority().getValue());
        for (var value : next.pack(100)) out.printf("block1 %s %d %d %d %d %d%n", BuiltInRegistries.BLOCK.getKey(value.type()), value.pos().getX(), value.pos().getY(), value.pos().getZ(), value.delay(), value.priority().getValue());
        for (var value : liquid.pack(100)) out.printf("fluid0 %s %d %d %d %d %d%n", BuiltInRegistries.FLUID.getKey(value.type()), value.pos().getX(), value.pos().getY(), value.pos().getZ(), value.delay(), value.priority().getValue());
        out.println("counter " + counter);
        out.println("END_RESULT");
    }
}
