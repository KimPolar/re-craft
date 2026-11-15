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
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.ticks.LevelChunkTicks;
import net.minecraft.world.ticks.SavedTick;

/** Verify the session's remaining block ticks with the 26.3 tick codec. */
public class AirSpeleothemSavedTickOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        net.minecraft.nbt.CompoundTag chunk;
        try (var input = new DataInputStream(Files.newInputStream(Path.of(args[0])))) {
            chunk = NbtIo.read((DataInput) input);
        }
        var codec = SavedTick.codec(BuiltInRegistries.BLOCK.byNameCodec());
        List<SavedTick<Block>> ticks = new ArrayList<>();
        for (var tag : chunk.getList("block_ticks").orElseThrow()) {
            ticks.add(codec.parse(NbtOps.INSTANCE, tag).getOrThrow());
        }
        var types = new Block[] {Blocks.POINTED_DRIPSTONE, Blocks.SAND, Blocks.VOID_AIR,
                                 Blocks.SULFUR_SPIKE, Blocks.AIR};
        var delays = new int[] {30, 3, 5, 40, 10};
        var priorities = new int[] {1, 0, -2, 2, 0};
        var position = new BlockPos(Integer.parseInt(args[1]), Integer.parseInt(args[2]),
                                    Integer.parseInt(args[3]));
        if (ticks.size() != types.length) throw new AssertionError(ticks);
        for (int i = 0; i < ticks.size(); i++) {
            var tick = ticks.get(i);
            if (tick.type() != types[i] || tick.delay() != delays[i]
                    || tick.priority().getValue() != priorities[i] || !tick.pos().equals(position))
                throw new AssertionError("saved tick mismatch: " + tick);
        }
        var queue = new LevelChunkTicks<Block>(ticks);
        queue.unpack(100L);
        if (!queue.pack(100L).equals(ticks)) throw new AssertionError("tick order changed");
        var overdue = queue.pack(125L);
        for (int i = 0; i < ticks.size(); i++) {
            var tick = overdue.get(i);
            if (tick.type() != types[i] || tick.delay() != delays[i] - 25
                    || tick.priority().getValue() != priorities[i] || !tick.pos().equals(position))
                throw new AssertionError("overdue tick mismatch: " + tick);
            if (!codec.parse(NbtOps.INSTANCE, codec.encodeStart(NbtOps.INSTANCE, tick).getOrThrow())
                    .getOrThrow().equals(tick)) throw new AssertionError("codec round trip changed tick");
        }
    }
}
