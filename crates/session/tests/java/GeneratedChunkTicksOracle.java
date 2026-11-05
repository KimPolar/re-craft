import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.world.ticks.*;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.ChunkPos;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Proxy;

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
        long[] counter={Long.MAX_VALUE-1};
        var level=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(proxy,method,arguments)->{
            return switch(method.getName()) {
                case "getGameTime" -> Long.MAX_VALUE;
                case "nextSubTickCount" -> counter[0]++;
                default -> InvocationHandler.invokeDefault(proxy,method,arguments);
            };
        });
        var blockTicks=new LevelTicks<String>(p->true);
        var fluidTicks=new LevelTicks<String>(p->true);
        var firstBlock=new LevelChunkTicks<String>(List.of(new SavedTick<>("minecraft:stone",A,0,TickPriority.NORMAL)));
        var secondFluid=new LevelChunkTicks<String>();
        blockTicks.addContainer(new ChunkPos(-2,3),firstBlock);
        fluidTicks.addContainer(new ChunkPos(-1,3),secondFluid);
        blockTicks.schedule(level.createTick(A,"minecraft:stone",1,TickPriority.HIGH));
        fluidTicks.schedule(level.createTick(new BlockPos(-16,3,50),"minecraft:water",3,TickPriority.LOW));
        blockTicks.schedule(level.createTick(new BlockPos(0,0,50),"minecraft:stone",0,TickPriority.NORMAL));
        blockTicks.schedule(level.createTick(B,"minecraft:sand",-1,TickPriority.HIGH));
        saved("region-block",firstBlock.pack(Long.MAX_VALUE));
        saved("region-fluid",secondFluid.pack(Long.MAX_VALUE));
        System.out.println("counter "+counter[0]);
        firstBlock.unpack(Long.MAX_VALUE);
        secondFluid.unpack(Long.MAX_VALUE);
        while (firstBlock.peek()!=null) polled(firstBlock.poll());
        while (secondFluid.peek()!=null) polled(secondFluid.poll());
        var collector = new LevelTicks<String>(p -> true);
        var c0 = new LevelChunkTicks<String>();
        var c1 = new LevelChunkTicks<String>();
        var c2 = new LevelChunkTicks<String>();
        collector.addContainer(new ChunkPos(0, 0), c0);
        collector.addContainer(new ChunkPos(1, 0), c1);
        collector.addContainer(new ChunkPos(2, 0), c2);
        c0.schedule(tick("minecraft:a0", new BlockPos(0, 0, 0), 40, TickPriority.NORMAL, 0));
        c0.schedule(tick("minecraft:a1", new BlockPos(1, 0, 0), 41, TickPriority.EXTREMELY_HIGH, 4));
        c1.schedule(tick("minecraft:b0", new BlockPos(16, 0, 0), 42, TickPriority.HIGH, 1));
        c2.schedule(tick("minecraft:c0", new BlockPos(32, 0, 0), 41, TickPriority.LOW, 2));
        c2.schedule(tick("minecraft:future", new BlockPos(33, 0, 0), 50, TickPriority.NORMAL, 7));
        collector.tick(42, 2, (p, type) -> {
            System.out.println("R1 " + type);
            if (type.equals("minecraft:b0")) {
                collector.schedule(tick("minecraft:new", new BlockPos(17, 0, 0), 42, TickPriority.EXTREMELY_HIGH, 5));
            }
        });
        collector.tick(42, 100, (p, type) -> System.out.println("R2 " + type));
        collector.tick(50, 100, (p, type) -> System.out.println("R3 " + type));
        var ties = new LevelTicks<String>(p -> true);
        for (int i = 0; i < 20; i++) {
            int x = i - 10, z = (i * 7) % 5 - 2;
            var container = new LevelChunkTicks<String>();
            container.schedule(tick("minecraft:t" + i + "a", new BlockPos(x * 16, 0, z * 16), 5, TickPriority.NORMAL, 0));
            container.schedule(tick("minecraft:t" + i + "b", new BlockPos(x * 16, 1, z * 16), 5, TickPriority.NORMAL, 0));
            ties.addContainer(new ChunkPos(x, z), container);
        }
        for (int round = 0; round < 14; round++) {
            int current = round;
            ties.tick(5, 3, (p, type) -> {
                System.out.println("E" + current + " " + type);
                if (type.equals("minecraft:t0a")) {
                    ties.schedule(tick("minecraft:callback", new BlockPos(-160, 2, -32), 5, TickPriority.HIGH, -1));
                }
            });
        }
        var collectBeforeRun = new LevelTicks<String>(p -> true);
        var water = new LevelChunkTicks<String>();
        water.schedule(tick("minecraft:water", new BlockPos(0, 0, 0), 0, TickPriority.NORMAL, 0));
        water.schedule(tick("minecraft:water", new BlockPos(1, 0, 0), 0, TickPriority.NORMAL, 1));
        collectBeforeRun.addContainer(new ChunkPos(0, 0), water);
        collectBeforeRun.tick(0, 10, (p, type) -> {
            System.out.println("D1 " + p.getX());
            if (p.getX() == 0) {
                collectBeforeRun.schedule(tick("minecraft:water", new BlockPos(1, 0, 0), 0, TickPriority.NORMAL, 2));
            }
        });
        collectBeforeRun.tick(0, 10, (p, type) -> System.out.println("D2 " + p.getX()));
        var streamed = new LevelTicks<String>(p -> true);
        @SuppressWarnings("unchecked")
        LevelChunkTicks<String>[] containers = new LevelChunkTicks[10];
        for (int i = 0; i < containers.length; i++) {
            var at = new BlockPos(i * 16, 0, 0);
            var pending = new SavedTick<String>("minecraft:p" + i, at, 0, TickPriority.NORMAL);
            var container = new LevelChunkTicks<String>(List.of(pending));
            container.schedule(tick("minecraft:q" + i, at.above(), 100, TickPriority.NORMAL, 0));
            containers[i] = container;
            streamed.addContainer(new ChunkPos(i, 0), container);
            container.unpack(100);
        }
        streamed.tick(100, 3, (p, type) -> System.out.println("U1 " + type));
        for (int i : new int[]{1, 4, 7}) streamed.removeContainer(new ChunkPos(i, 0));
        streamed.tick(100, 4, (p, type) -> System.out.println("U2 " + type));
        for (int i : new int[]{7, 1, 4}) streamed.addContainer(new ChunkPos(i, 0), containers[i]);
        streamed.tick(100, 100, (p, type) -> System.out.println("U3 " + type));
        var replaced = new LevelTicks<String>(p -> true);
        var original = new LevelChunkTicks<String>();
        original.schedule(tick("minecraft:old", new BlockPos(0, 0, 16), 2, TickPriority.NORMAL, 0));
        var other = new LevelChunkTicks<String>();
        other.schedule(tick("minecraft:other", new BlockPos(16, 0, 16), 2, TickPriority.NORMAL, 0));
        replaced.addContainer(new ChunkPos(0, 1), original);
        replaced.addContainer(new ChunkPos(1, 1), other);
        replaced.addContainer(new ChunkPos(0, 1), new LevelChunkTicks<String>());
        replaced.tick(2, 100, (p, type) -> System.out.println("X1 " + type));
        replaced.schedule(tick("minecraft:new", new BlockPos(0, 1, 16), 3, TickPriority.NORMAL, 2));
        replaced.tick(3, 100, (p, type) -> System.out.println("X2 " + type));
        var history = new LevelTicks<String>(p -> true);
        @SuppressWarnings("unchecked")
        LevelChunkTicks<String>[] historyChunks = new LevelChunkTicks[30];
        int[] historyX = new int[30];
        for (int x = 1, count = 0; count < 30; x++) {
            if ((it.unimi.dsi.fastutil.HashCommon.mix(ChunkPos.pack(x, 0)) & 31) == 31) historyX[count++] = x;
        }
        for (int i = 0; i < 30; i++) {
            int x = historyX[i];
            var pending = new SavedTick<String>("minecraft:hp" + i, new BlockPos(x * 16, 0, 0), 0, TickPriority.NORMAL);
            historyChunks[i] = new LevelChunkTicks<String>(List.of(pending));
            history.addContainer(new ChunkPos(x, 0), historyChunks[i]);
        }
        for (int j = 0; j < 30; j++) {
            int i = (j * 17) % 30, x = historyX[i];
            history.schedule(tick("minecraft:hq" + i, new BlockPos(x * 16, 1, 0), 101, TickPriority.NORMAL, j));
        }
        for (int i = 29; i >= 0; i--) historyChunks[i].unpack(100);
        for (int round = 0; round < 10; round++) {
            history.tick(100, 3, (p, type) -> System.out.println("H1 " + type));
        }
        history.tick(101, 100, (p, type) -> System.out.println("H2 " + type));
        var dynamic = new LevelTicks<String>(p -> true);
        var first = new LevelChunkTicks<String>(List.of(new SavedTick<String>(
            "minecraft:pending0", new BlockPos(0, 0, 0), 0, TickPriority.NORMAL)));
        dynamic.addContainer(new ChunkPos(0, 0), first);
        dynamic.schedule(tick("minecraft:live0", new BlockPos(0, 1, 0), 101, TickPriority.NORMAL, 5));
        var second = new LevelChunkTicks<String>(List.of(new SavedTick<String>(
            "minecraft:pending1", new BlockPos(16, 0, 0), 0, TickPriority.NORMAL)));
        second.schedule(tick("minecraft:existing1", new BlockPos(16, 1, 0), 102, TickPriority.NORMAL, 7));
        dynamic.addContainer(new ChunkPos(1, 0), second);
        dynamic.schedule(tick("minecraft:live1", new BlockPos(16, 2, 0), 101, TickPriority.NORMAL, 6));
        first.unpack(100);
        second.unpack(100);
        for (long time = 100; time <= 102; time++) {
            final long now = time;
            dynamic.tick(time, 100, (p, type) -> System.out.println("D " + now + " " + type));
        }
    }
}
