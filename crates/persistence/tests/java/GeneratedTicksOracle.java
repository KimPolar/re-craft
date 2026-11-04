import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.ticks.*;

public class GeneratedTicksOracle {
    static <T> void schedule(ProtoChunkTicks<T> ticks, T type, int x, int y, int z, TickPriority priority, long n) {
        ticks.schedule(new ScheduledTick<>(type, new BlockPos(x,y,z), n * 11, priority, n));
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var blockTicks = new ProtoChunkTicks<Block>();
        schedule(blockTicks, Blocks.AIR, -32, -65, 63, TickPriority.HIGH, 1);
        schedule(blockTicks, Blocks.SAND, -31, 15, 48, TickPriority.EXTREMELY_HIGH, 2);
        schedule(blockTicks, Blocks.WATER, -31, 15, 48, TickPriority.EXTREMELY_LOW, 3);
        schedule(blockTicks, Blocks.SAND, -17, 256, 50, TickPriority.NORMAL, 4);
        schedule(blockTicks, Blocks.SAND, -31, 15, 48, TickPriority.LOW, 5);
        schedule(blockTicks, Blocks.DIRT, -32, 16, 48, TickPriority.LOW, 10);
        var fluidTicks = new ProtoChunkTicks<Fluid>();
        schedule(fluidTicks, Fluids.WATER, -30, 1, 52, TickPriority.VERY_LOW, 6);
        schedule(fluidTicks, Fluids.LAVA, -30, 1, 52, TickPriority.VERY_HIGH, 7);
        schedule(fluidTicks, Fluids.FLOWING_WATER, -17, 320, 63, TickPriority.NORMAL, 8);
        schedule(fluidTicks, Fluids.WATER, -30, 1, 52, TickPriority.EXTREMELY_HIGH, 9);
        var blockCodec = SavedTick.codec(BuiltInRegistries.BLOCK.byNameCodec()).listOf();
        var fluidCodec = SavedTick.codec(BuiltInRegistries.FLUID.byNameCodec()).listOf();
        var actual = NbtIo.read(Path.of(args[0]));
        var expectedBlocks = blockCodec.encodeStart(NbtOps.INSTANCE,blockTicks.pack(100)).getOrThrow();
        var expectedFluids = fluidCodec.encodeStart(NbtOps.INSTANCE,fluidTicks.pack(100)).getOrThrow();
        if (!actual.get("block_ticks").equals(expectedBlocks) || !actual.get("fluid_ticks").equals(expectedFluids))
            throw new AssertionError("ticks differ: " + actual + " expected " + expectedBlocks + " / " + expectedFluids);
        var empty = NbtIo.read(Path.of(args[1]));
        if (!empty.get("block_ticks").equals(blockCodec.encodeStart(NbtOps.INSTANCE,java.util.List.of()).getOrThrow()) ||
            !empty.get("fluid_ticks").equals(fluidCodec.encodeStart(NbtOps.INSTANCE,java.util.List.of()).getOrThrow()))
            throw new AssertionError("empty tick lists differ");
    }
}
