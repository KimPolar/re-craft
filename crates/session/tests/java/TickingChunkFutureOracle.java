import java.util.*;
import java.util.concurrent.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.*;
import net.minecraft.util.thread.BlockableEventLoop;
import net.minecraft.world.level.*;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;

/** Observe the real ChunkMap 3x3 ticking future with a controlled executor. */
public class TickingChunkFutureOracle {
    static sun.misc.Unsafe unsafe;
    static final List<String> events = new ArrayList<>();

    static class Executor extends BlockableEventLoop<Runnable> {
        final ArrayDeque<Runnable> queued = new ArrayDeque<>();
        Executor() { super("ticking-oracle", false); }
        @Override protected boolean shouldRun(Runnable task) { return true; }
        @Override protected Thread getRunningThread() { return null; }
        @Override public Runnable wrapRunnable(Runnable task) { return task; }
        @Override public void execute(Runnable task) { queued.add(task); }
        void drain() { while (!queued.isEmpty()) queued.remove().run(); }
    }

    static class Level extends ServerLevel {
        Level() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public void startTickingChunk(LevelChunk chunk) {
            var label = ((Chunk) chunk).label;
            events.add(label == null ? "unpack" : "unpack:" + label);
        }
    }

    static class Chunk extends LevelChunk {
        boolean fail;
        String label;
        Chunk() { super((net.minecraft.world.level.Level) null, new ChunkPos(0, 0)); }
        @Override public void postProcessGeneration(ServerLevel level) {
            events.add(label == null ? "post" : "post:" + label);
            if (fail) throw new IllegalStateException("post failed");
        }
    }

    static class Holder extends ChunkHolder {
        ChunkPos position;
        CompletableFuture<ChunkResult<ChunkAccess>> full;
        CompletableFuture<Object> send;
        Holder() { super(null, 0, null, null, null, null); }
        @Override public ChunkPos getPos() { return position; }
        @Override public CompletableFuture<ChunkResult<ChunkAccess>> scheduleChunkGenerationTask(
                ChunkStatus status, ChunkMap map) {
            if (status != ChunkStatus.FULL) throw new AssertionError("not FULL");
            events.add("request:" + position.x() + "," + position.z());
            return full;
        }
        @Override public CompletableFuture<?> getSendSyncFuture() { return send; }
    }

    static class Map extends ChunkMap {
        java.util.Map<Long, Holder> holders;
        Map() { super(null, null, null, null, null, null, null, null, null, null, 0, false); }
        @Override public ChunkHolder getUpdatingChunkIfPresent(long pos) { return holders.get(pos); }
    }

    static void set(Class<?> type, Object object, String name, Object value) throws Exception {
        var field = type.getDeclaredField(name);
        field.setAccessible(true);
        field.set(object, value);
    }

    static class Fixture {
        Map map;
        Executor executor;
        Holder[] holders = new Holder[9];
        Chunk chunk;
        Fixture(int missing) throws Exception {
            map = (Map) unsafe.allocateInstance(Map.class);
            map.holders = new HashMap<>();
            executor = new Executor();
            set(ChunkMap.class, map, "mainThreadExecutor", executor);
            set(ChunkMap.class, map, "level", unsafe.allocateInstance(Level.class));
            chunk = (Chunk) unsafe.allocateInstance(Chunk.class);
            for (int i = 0; i < 9; i++) {
                var holder = (Holder) unsafe.allocateInstance(Holder.class);
                holder.position = new ChunkPos(i % 3 - 1, i / 3 - 1);
                holder.full = new CompletableFuture<>();
                holder.send = new CompletableFuture<>();
                holders[i] = holder;
                if (i != missing) map.holders.put(holder.position.pack(), holder);
            }
        }
        CompletableFuture<ChunkResult<LevelChunk>> start() { return map.prepareTickingChunk(holders[4]); }
        void complete(int index, boolean success) {
            holders[index].full.complete(success ? ChunkResult.of(chunk) : ChunkResult.error("bad" + index));
        }
        String snapshot(CompletableFuture<ChunkResult<LevelChunk>> future) {
            String state = !future.isDone() ? "waiting" : future.isCompletedExceptionally() ? "exception"
                    : future.join().isSuccess() ? "success" : "error";
            return state + "/" + executor.queued.size() + "/" + String.join(",", events);
        }
    }

    static void run(String name) throws Exception {
        events.clear();
        var fixture = new Fixture(name.equals("missing") ? 1 : -1);
        var future = fixture.start();
        var initial = fixture.snapshot(future);
        events.clear();
        if (!name.equals("missing")) {
            for (int i = 0; i < 8; i++) {
                if (name.equals("exception") && i == 2) {
                    fixture.holders[i].full.completeExceptionally(new IllegalStateException("generation failed"));
                } else {
                    fixture.complete(i, !name.equals("failure") || i != 2);
                }
            }
            if (name.equals("cancel-before")) future.complete(ChunkHolder.UNLOADED_LEVEL_CHUNK);
        }
        var beforeLast = fixture.snapshot(future);
        if (!name.equals("missing")) fixture.complete(8, true);
        var queued = fixture.snapshot(future);
        if (name.equals("cancel-after")) future.complete(ChunkHolder.UNLOADED_LEVEL_CHUNK);
        if (name.equals("throw-post")) fixture.chunk.fail = true;
        if (name.equals("send-ready")) fixture.holders[4].send.complete(null);
        fixture.executor.drain();
        System.out.println(name + "|" + initial + "|" + beforeLast + "|" + queued + "|" + fixture.snapshot(future));
    }

    static void runOverlap(int sharedX, int sharedZ, boolean reverse) throws Exception {
        events.clear();
        var map = (Map) unsafe.allocateInstance(Map.class);
        map.holders = new HashMap<>();
        var executor = new Executor();
        set(ChunkMap.class, map, "mainThreadExecutor", executor);
        set(ChunkMap.class, map, "level", unsafe.allocateInstance(Level.class));
        var chunks = new java.util.LinkedHashMap<Holder, Chunk>();
        for (int z = -1; z <= 1; z++) for (int x = -1; x <= 2; x++) {
            var holder = (Holder) unsafe.allocateInstance(Holder.class);
            holder.position = new ChunkPos(x, z);
            holder.full = new CompletableFuture<>();
            holder.send = new CompletableFuture<>();
            var chunk = (Chunk) unsafe.allocateInstance(Chunk.class);
            chunk.label = x + "," + z;
            map.holders.put(holder.position.pack(), holder);
            chunks.put(holder, chunk);
        }
        CompletableFuture<ChunkResult<LevelChunk>> first, second;
        if (reverse) {
            second = map.prepareTickingChunk(map.holders.get(ChunkPos.pack(1, 0)));
            first = map.prepareTickingChunk(map.holders.get(ChunkPos.pack(0, 0)));
        } else {
            first = map.prepareTickingChunk(map.holders.get(ChunkPos.pack(0, 0)));
            second = map.prepareTickingChunk(map.holders.get(ChunkPos.pack(1, 0)));
        }
        events.clear();
        for (var entry : chunks.entrySet()) {
            if (entry.getKey().position.equals(new ChunkPos(sharedX, sharedZ))) continue;
            entry.getKey().full.complete(ChunkResult.of(entry.getValue()));
        }
        int before = executor.queued.size();
        var shared = map.holders.get(ChunkPos.pack(sharedX, sharedZ));
        shared.full.complete(ChunkResult.of(chunks.get(shared)));
        int queued = executor.queued.size();
        executor.drain();
        System.out.println("overlap:" + sharedX + "," + sharedZ + ":" + reverse + "|" + before + "|" + queued + "|" + String.join(",", events)
                + "|" + first.join().isSuccess() + "," + second.join().isSuccess());
    }

    static void shuffle(List<Integer> values, int seed) {
        long state = seed + 1L;
        for (int i = values.size() - 1; i > 0; i--) {
            state = state * 6364136223846793005L + 1442695040888963407L;
            int other = (int) Long.remainderUnsigned(state, i + 1L);
            Collections.swap(values, i, other);
        }
    }

    static void runBatch(int number) throws Exception {
        events.clear();
        var map = (Map) unsafe.allocateInstance(Map.class);
        map.holders = new HashMap<>();
        var executor = new Executor();
        set(ChunkMap.class, map, "mainThreadExecutor", executor);
        set(ChunkMap.class, map, "level", unsafe.allocateInstance(Level.class));
        var holders = new Holder[16];
        var chunks = new Chunk[16];
        for (int z = -1; z <= 2; z++) for (int x = -1; x <= 2; x++) {
            int index = (z + 1) * 4 + x + 1;
            var holder = (Holder) unsafe.allocateInstance(Holder.class);
            holder.position = new ChunkPos(x, z);
            holder.full = new CompletableFuture<>();
            holder.send = new CompletableFuture<>();
            var chunk = (Chunk) unsafe.allocateInstance(Chunk.class);
            chunk.label = x + "," + z;
            holders[index] = holder;
            chunks[index] = chunk;
            map.holders.put(holder.position.pack(), holder);
        }
        @SuppressWarnings("unchecked")
        var futures = (CompletableFuture<ChunkResult<LevelChunk>>[]) new CompletableFuture<?>[4];
        var order = new ArrayList<>(List.of(0, 1, 2, 3));
        shuffle(order, number * 3 + 1);
        for (int index : order) {
            futures[index] = map.prepareTickingChunk(map.holders.get(ChunkPos.pack(index % 2, index / 2)));
        }
        events.clear();
        var completions = new ArrayList<Integer>();
        for (int i = 0; i < 16; i++) completions.add(i);
        shuffle(completions, number * 3 + 2);
        var counts = new ArrayList<String>();
        for (int i = 0; i < completions.size(); i++) {
            int index = completions.get(i);
            if (i == number % 16 && number % 3 == 1) {
                holders[index].full.complete(ChunkResult.error("unavailable"));
            } else if (i == number % 16 && number % 3 == 2) {
                holders[index].full.completeExceptionally(new IllegalStateException("failed"));
            } else {
                holders[index].full.complete(ChunkResult.of(chunks[index]));
            }
            if (i == 7 && number % 4 == 0) futures[(number / 4) % 4].complete(ChunkHolder.UNLOADED_LEVEL_CHUNK);
            counts.add(Integer.toString(executor.queued.size()));
        }
        executor.drain();
        var states = new ArrayList<String>();
        for (var future : futures) {
            states.add(future.isCompletedExceptionally() ? "E" : future.join().isSuccess() ? "S" : "U");
        }
        System.out.println("batch:" + number + "|" + String.join(",", counts) + "|"
                + String.join(",", events) + "|" + String.join(",", states));
    }

    public static void main(String[] args) throws Exception {
        var out = System.out;
        var errors = System.err;
        try {
            SharedConstants.tryDetectVersion();
            Bootstrap.bootStrap();
            var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
            field.setAccessible(true);
            unsafe = (sun.misc.Unsafe) field.get(null);
            System.setOut(out);
            out.println("BEGIN_RESULT");
            for (var name : List.of("missing", "success", "failure", "exception", "cancel-before", "cancel-after", "throw-post", "send-ready")) run(name);
            for (int z = -1; z <= 1; z++) for (int x = 0; x <= 1; x++) {
                runOverlap(x, z, false);
                runOverlap(x, z, true);
            }
            for (int number = 0; number < 24; number++) runBatch(number);
            out.println("END_RESULT");
        } catch (Throwable error) {
            error.printStackTrace(errors);
            throw error;
        }
    }
}
