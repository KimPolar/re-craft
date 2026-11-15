import java.io.DataInput;
import java.io.DataInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.ticks.LevelChunkTicks;
import net.minecraft.world.ticks.SavedTick;

/** Verify both saved tick queues after a Rust bubble-column update. */
public class BubbleEffectSavedTickOracle {
    static <T> void verify(List<SavedTick<T>> ticks, T[] types, BlockPos[] positions,
                           int[] delays, int[] priorities) {
        if (ticks.size() != types.length) throw new AssertionError(ticks);
        for (int i = 0; i < ticks.size(); i++) {
            var tick = ticks.get(i);
            if (tick.type() != types[i] || !tick.pos().equals(positions[i])
                    || tick.delay() != delays[i] || tick.priority().getValue() != priorities[i])
                throw new AssertionError("saved tick mismatch: " + tick);
        }
        var queue = new LevelChunkTicks<T>(ticks);
        queue.unpack(100L);
        if (!queue.pack(100L).equals(ticks)) throw new AssertionError("tick order changed");
        var overdue = queue.pack(125L);
        for (int i = 0; i < ticks.size(); i++) {
            var tick = overdue.get(i);
            if (tick.type() != types[i] || !tick.pos().equals(positions[i])
                    || tick.delay() != delays[i] - 25 || tick.priority().getValue() != priorities[i])
                throw new AssertionError("overdue tick mismatch: " + tick);
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        net.minecraft.nbt.CompoundTag chunk;
        try (var input = new DataInputStream(Files.newInputStream(Path.of(args[0])))) {
            chunk = NbtIo.read((DataInput) input);
        }
        var center = new BlockPos(Integer.parseInt(args[1]), Integer.parseInt(args[2]),
                                  Integer.parseInt(args[3]));
        var blockCodec = SavedTick.codec(BuiltInRegistries.BLOCK.byNameCodec());
        var blockTicks = new ArrayList<net.minecraft.world.ticks.SavedTick<net.minecraft.world.level.block.Block>>();
        for (var tag : chunk.getList("block_ticks").orElseThrow())
            blockTicks.add(blockCodec.parse(NbtOps.INSTANCE, tag).getOrThrow());
        verify(blockTicks, new net.minecraft.world.level.block.Block[] {
                    Blocks.SAND, Blocks.WATER, Blocks.BUBBLE_COLUMN},
                new BlockPos[] {center, center, center.east()},
                new int[] {20, 10, 5}, new int[] {0, 1, 0});

        var fluidCodec = SavedTick.codec(BuiltInRegistries.FLUID.byNameCodec());
        var fluidTicks = new ArrayList<net.minecraft.world.ticks.SavedTick<net.minecraft.world.level.material.Fluid>>();
        for (var tag : chunk.getList("fluid_ticks").orElseThrow())
            fluidTicks.add(fluidCodec.parse(NbtOps.INSTANCE, tag).getOrThrow());
        verify(fluidTicks, new net.minecraft.world.level.material.Fluid[] {
                    Fluids.WATER, Fluids.FLOWING_WATER, Fluids.WATER, Fluids.WATER},
                new BlockPos[] {center.east(), center.west(), center.above(), center},
                new int[] {13, 15, 5, 5}, new int[] {-2, 1, 0, 0});
    }
}
