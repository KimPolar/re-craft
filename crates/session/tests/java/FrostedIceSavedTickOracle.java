import java.io.DataInput;
import java.io.DataInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.ticks.LevelChunkTicks;
import net.minecraft.world.ticks.SavedTick;
import net.minecraft.world.ticks.ScheduledTick;
import net.minecraft.world.ticks.TickPriority;

/** Read the Rust chunk's block-tick list with Minecraft 26.3. */
public class FrostedIceSavedTickOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var root = NbtIo.read((DataInput)new DataInputStream(Files.newInputStream(Path.of(args[0]))));
        var codec = SavedTick.codec(BuiltInRegistries.BLOCK.byNameCodec());
        List<SavedTick<Block>> ticks = new ArrayList<>();
        for (var value : root.getList("block_ticks").orElseThrow()) {
            ticks.add(codec.parse(NbtOps.INSTANCE, value).getOrThrow());
        }
        var types = new Block[] {Blocks.FROSTED_ICE, Blocks.SAND, Blocks.FROSTED_ICE, Blocks.WATER};
        var delays = new int[] {20, 3, 5, 6};
        var priorities = new TickPriority[] {TickPriority.HIGH, TickPriority.NORMAL,
                                             TickPriority.LOW, TickPriority.VERY_HIGH};
        if (ticks.size() != types.length) throw new AssertionError(ticks);
        for (int i = 0; i < ticks.size(); i++) {
            if (ticks.get(i).type() != types[i] || ticks.get(i).delay() != delays[i]
                    || ticks.get(i).priority() != priorities[i]) throw new AssertionError(ticks);
        }
        var chunk = new LevelChunkTicks<Block>(ticks);
        chunk.unpack(100L);
        if (!chunk.pack(100L).equals(ticks)) throw new AssertionError("unpack/pack changed ticks");
        var overdue = chunk.pack(125L);
        for (int i = 0; i < ticks.size(); i++) {
            var tick = overdue.get(i);
            if (tick.delay() != delays[i] - 25 || tick.type() != types[i]
                    || !tick.pos().equals(ticks.get(i).pos()) || tick.priority() != priorities[i])
                throw new AssertionError("overdue tick changed: " + tick);
            if (!codec.parse(NbtOps.INSTANCE, codec.encodeStart(NbtOps.INSTANCE, tick).getOrThrow())
                    .getOrThrow().equals(tick)) throw new AssertionError("NBT round trip changed tick");
        }
        if (chunk.poll().type() != Blocks.SAND || chunk.poll().type() != Blocks.FROSTED_ICE
                || chunk.poll().type() != Blocks.WATER) throw new AssertionError("execution order changed");
        var remaining = ticks.get(0);
        if (chunk.hasScheduledTick(remaining.pos(), Blocks.FROSTED_ICE))
            throw new AssertionError("poll did not remove the shared identity");
        chunk.schedule(new ScheduledTick<>(Blocks.FROSTED_ICE, remaining.pos(), 130L, 10L));
        if (chunk.count() != 2) throw new AssertionError("duplicate was not retained");
    }
}
