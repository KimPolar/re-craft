import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.world.ticks.*;

public class GeneratedChunkTicksOracle {
    static final BlockPos A = new BlockPos(-31, 0, 50);
    static final BlockPos B = new BlockPos(-30, 1, 50);
    static final BlockPos C = new BlockPos(-29, 2, 50);
    static String pos(BlockPos p) { return p.getX()+","+p.getY()+","+p.getZ(); }
    static void saved(String label, List<SavedTick<String>> list) {
        System.out.print(label);
        for (var t : list) System.out.print(" "+t.type()+"/"+pos(t.pos())+"/"+t.delay()+"/"+t.priority().getValue());
        System.out.println();
    }
    static void polled(ScheduledTick<String> t) {
        System.out.println(t == null ? "P null" : "P "+t.type()+"/"+pos(t.pos())+"/"+t.triggerTick()+"/"+t.priority().getValue()+"/"+t.subTickOrder());
    }
    static ScheduledTick<String> tick(String kind, BlockPos p, long time, TickPriority priority, long sub) {
        return new ScheduledTick<>(kind, p, time, priority, sub);
    }
    public static void main(String[] args) {
        var proto = new ProtoChunkTicks<String>();
        proto.schedule(tick("minecraft:water", A, 300, TickPriority.HIGH, 99));
        proto.schedule(tick("minecraft:lava", B, 400, TickPriority.LOW, 98));
        proto.schedule(tick("minecraft:water", A, 1, TickPriority.EXTREMELY_HIGH, 0));
        var full = new LevelChunkTicks<String>(proto.scheduledTicks());
        full.schedule(tick("minecraft:water", A, 1, TickPriority.NORMAL, 1));
        full.schedule(tick("minecraft:water", C, 45, TickPriority.NORMAL, 7));
        full.schedule(tick("minecraft:lava", C, 45, TickPriority.NORMAL, 7));
        saved("before", full.pack(42));
        polled(full.peek());
        full.unpack(42);
        saved("after", full.pack(42));
        while (full.peek()!=null) polled(full.poll());
        polled(full.poll());
        var repeated = new SavedTick<String>("minecraft:water", A, -2, TickPriority.NORMAL);
        var duplicates = new LevelChunkTicks<String>(List.of(repeated,repeated));
        duplicates.unpack(Long.MAX_VALUE);
        polled(duplicates.poll());
        duplicates.schedule(tick("minecraft:water", A, Long.MIN_VALUE, TickPriority.HIGH, 6));
        saved("duplicates", duplicates.pack(Long.MAX_VALUE));
        while (duplicates.peek()!=null) polled(duplicates.poll());
        var many = new LevelChunkTicks<String>();
        for (int i=0;i<80;i++) {
            var p = new BlockPos(-32+i%16,i%5,48+(i*7)%16);
            many.schedule(tick(("minecraft:k"+(i%4)).intern(),p,100+(i*13)%7,TickPriority.byValue((i*5)%7-3),(i*3)%9-4));
        }
        saved("many",many.pack(100));
        while (many.peek()!=null) polled(many.poll());
    }
}
