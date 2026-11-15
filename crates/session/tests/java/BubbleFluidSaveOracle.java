import java.io.DataInput;
import java.io.DataInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.BubbleColumnBlock;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.Strategy;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.ticks.LevelChunkTicks;
import net.minecraft.world.ticks.SavedTick;

/** Read the Rust draft using 26.3's chunk palette and scheduled-tick codecs. */
public class BubbleFluidSaveOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        CompoundTag chunk;
        try (var input = new DataInputStream(Files.newInputStream(Path.of(args[0])))) {
            chunk = NbtIo.read((DataInput) input);
        }
        var center = new BlockPos(Integer.parseInt(args[1]), Integer.parseInt(args[2]),
                                  Integer.parseInt(args[3]));
        var section = (CompoundTag) chunk.getList("sections").orElseThrow().get(0);
        var paletteCodec = PalettedContainer.codecRW(BlockState.CODEC,
                Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState());
        var states = paletteCodec.parse(NbtOps.INSTANCE, section.getCompound("block_states").orElseThrow()).getOrThrow();
        var water = Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 1);
        var bubble = Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN, true);
        if (states.get(8, 10, 7) != water || states.get(8, 10, 8) != bubble
                || states.get(8, 9, 8) != Blocks.STONE.defaultBlockState())
            throw new AssertionError("saved block states changed");
        var encoded = paletteCodec.encodeStart(NbtOps.INSTANCE, states).getOrThrow();
        var reread = paletteCodec.parse(NbtOps.INSTANCE, encoded).getOrThrow();
        if (reread.get(8, 10, 7) != water || reread.get(8, 10, 8) != bubble)
            throw new AssertionError("palette round trip changed block states");
        if (!chunk.getList("block_ticks").orElseThrow().isEmpty())
            throw new AssertionError("unexpected block ticks");
        var codec = SavedTick.codec(BuiltInRegistries.FLUID.byNameCodec());
        var ticks = new ArrayList<SavedTick<Fluid>>();
        for (var tag : chunk.getList("fluid_ticks").orElseThrow())
            ticks.add(codec.parse(NbtOps.INSTANCE, tag).getOrThrow());
        var types = new Fluid[] {Fluids.FLOWING_WATER, Fluids.WATER};
        var positions = new BlockPos[] {center.north(), center};
        if (ticks.size() != types.length) throw new AssertionError(ticks);
        for (int i = 0; i < ticks.size(); i++) {
            var tick = ticks.get(i);
            if (tick.type() != types[i] || !tick.pos().equals(positions[i])
                    || tick.delay() != 5 || tick.priority().getValue() != 0)
                throw new AssertionError("saved fluid tick mismatch: " + tick);
            if (!codec.parse(NbtOps.INSTANCE, codec.encodeStart(NbtOps.INSTANCE, tick).getOrThrow())
                    .getOrThrow().equals(tick)) throw new AssertionError("tick codec round trip changed tick");
        }
        var queue = new LevelChunkTicks<Fluid>(ticks);
        queue.unpack(100L);
        if (!queue.pack(100L).equals(ticks)) throw new AssertionError("fluid tick order changed");
    }
}
