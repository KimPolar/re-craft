import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.ticks.*;

public class BubbleSavedTickOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var root = NbtIo.read((DataInput)new DataInputStream(Files.newInputStream(Path.of(args[0]))));
        var list = root.getList("block_ticks").orElseThrow();
        var codec = SavedTick.codec(BuiltInRegistries.BLOCK.byNameCodec());
        List<SavedTick<Block>> ticks = new ArrayList<>();
        for (var value : list) ticks.add(codec.parse(NbtOps.INSTANCE,value).getOrThrow());
        if (ticks.size()!=4 || ticks.get(0).type()!=Blocks.WATER || ticks.get(1).type()!=Blocks.SAND || ticks.get(2).type()!=Blocks.BUBBLE_COLUMN || ticks.get(3).type()!=Blocks.WATER) throw new AssertionError(ticks);
        int[] delay={20,3,5,6}; TickPriority[] priorities={TickPriority.HIGH,TickPriority.NORMAL,TickPriority.LOW,TickPriority.VERY_HIGH};
        for(int i=0;i<4;i++) if(ticks.get(i).delay()!=delay[i] || ticks.get(i).priority()!=priorities[i]) throw new AssertionError(ticks);
        var chunk = new LevelChunkTicks<Block>(ticks);
        chunk.unpack(100L);
        if(!chunk.pack(100L).equals(ticks)) throw new AssertionError("unpack/pack changed tick order or data: "+chunk.pack(100L));
        if(chunk.poll().type()!=Blocks.SAND || chunk.poll().type()!=Blocks.BUBBLE_COLUMN || chunk.poll().type()!=Blocks.WATER) throw new AssertionError("unexpected execution order");
        var remaining=ticks.get(0);
        if(chunk.hasScheduledTick(remaining.pos(),Blocks.WATER)) throw new AssertionError("duplicate still present in uniqueness set after poll");
        chunk.schedule(new ScheduledTick<>(Blocks.WATER,remaining.pos(),130L,10L));
        if(chunk.count()!=2) throw new AssertionError("new water tick was not scheduled alongside remaining duplicate");
    }
}
