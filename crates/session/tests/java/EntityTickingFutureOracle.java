import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.ChunkResult;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.thread.BlockableEventLoop;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import sun.misc.Unsafe;

/** Observe 26.3 entity-ticking and overlapping block-ticking futures. */
public class EntityTickingFutureOracle {
    static Unsafe unsafe;
    static final List<String> events = new ArrayList<>();

    static class Loop extends BlockableEventLoop<Runnable> {
        final ArrayDeque<Runnable> tasks = new ArrayDeque<>();
        Loop() { super("entity-future-oracle", false); }
        @Override protected boolean shouldRun(Runnable task) { return true; }
        @Override protected Thread getRunningThread() { return null; }
        @Override public Runnable wrapRunnable(Runnable task) { return task; }
        @Override public void execute(Runnable task) { events.add("block-queued"); tasks.add(task); }
    }
    static class Holder extends ChunkHolder {
        ChunkPos position;
        CompletableFuture<ChunkResult<ChunkAccess>> full;
        Holder() { super(null, 0, null, null, null, null); }
        @Override public ChunkPos getPos() { return position; }
        @Override public CompletableFuture<ChunkResult<ChunkAccess>> scheduleChunkGenerationTask(
                ChunkStatus status, ChunkMap map) {
            if (status != ChunkStatus.FULL) throw new AssertionError("expected FULL");
            events.add("request:" + position.x() + "," + position.z());
            return full;
        }
    }
    static class MapView extends ChunkMap {
        Map<Long, Holder> holders;
        MapView() { super(null, null, null, null, null, null, null, null, null, null, 0, false); }
        @Override public ChunkHolder getUpdatingChunkIfPresent(long pos) { return holders.get(pos); }
    }
    static void set(Class<?> type, Object object, String name, Object value) throws Exception {
        var field = type.getDeclaredField(name);
        field.setAccessible(true);
        field.set(object, value);
    }
    static String state(CompletableFuture<ChunkResult<LevelChunk>> future) {
        if (!future.isDone()) return "waiting";
        if (future.isCompletedExceptionally()) return "exception";
        return future.join().isSuccess() ? "available" : "unavailable";
    }
    static class Fixture {
        MapView map;
        Loop loop;
        Holder[] holders = new Holder[25];
        LevelChunk chunk;
        Fixture(int missing) throws Exception {
            map = (MapView) unsafe.allocateInstance(MapView.class);
            map.holders = new HashMap<>();
            loop = new Loop();
            set(ChunkMap.class, map, "mainThreadExecutor", loop);
            set(ChunkMap.class, map, "level", unsafe.allocateInstance(ServerLevel.class));
            chunk = (LevelChunk) unsafe.allocateInstance(LevelChunk.class);
            for (int i = 0; i < 25; i++) {
                var holder = (Holder) unsafe.allocateInstance(Holder.class);
                holder.position = new ChunkPos(i % 5 - 2, i / 5 - 2);
                holder.full = new CompletableFuture<>();
                holders[i] = holder;
                if (i != missing) map.holders.put(holder.position.pack(), holder);
            }
        }
        void complete(int index, String result) {
            var future = holders[index].full;
            if (result.equals("exception")) future.completeExceptionally(new IllegalStateException("failed"));
            else future.complete(result.equals("unavailable") ? ChunkResult.error("missing") : ChunkResult.of(chunk));
        }
    }
    static void basic(String name) throws Exception {
        events.clear();
        var fixture = new Fixture(name.equals("missing") ? 1 : -1);
        if (name.equals("shared")) fixture.holders[22].full = fixture.holders[2].full;
        if (name.equals("ready")) for (int i = 0; i < 25; i++) fixture.complete(i, "full");
        var future = fixture.map.prepareEntityTickingChunk(fixture.holders[12]);
        String requested = String.join(";", events);
        events.clear();
        future.thenAccept(result -> events.add("listener:A"));
        future.thenAccept(result -> events.add("listener:B"));
        future.thenAccept(result -> events.add("listener:C"));
        future.thenAccept(result -> events.add("listener:D"));
        String initial = state(future);
        if (!name.equals("missing") && !name.equals("ready")) {
            for (int i = 0; i < 24; i++) {
                String result = i == 2 && name.equals("unavailable") ? "unavailable"
                    : i == 2 && name.equals("exception") ? "exception" : "full";
                fixture.complete(i, result);
            }
        }
        String beforeLast = state(future);
        boolean cancelled = false;
        if (name.equals("cancel")) cancelled = future.complete(ChunkHolder.UNLOADED_LEVEL_CHUNK);
        if (!name.equals("missing") && !name.equals("ready")) fixture.complete(24, "full");
        System.out.println("basic:" + name + "|" + requested + "|" + initial + "|" + beforeLast
            + "|" + cancelled + "|" + state(future) + "|" + String.join(";", events));
    }
    static void overlap(boolean entityFirst, int sharedIndex, boolean duplicate) throws Exception {
        events.clear();
        var fixture = new Fixture(-1);
        if (duplicate) fixture.holders[0].full = fixture.holders[sharedIndex].full;
        CompletableFuture<ChunkResult<LevelChunk>> entity, block;
        if (entityFirst) {
            entity = fixture.map.prepareEntityTickingChunk(fixture.holders[12]);
            block = fixture.map.prepareTickingChunk(fixture.holders[12]);
        } else {
            block = fixture.map.prepareTickingChunk(fixture.holders[12]);
            entity = fixture.map.prepareEntityTickingChunk(fixture.holders[12]);
        }
        events.clear();
        entity.thenAccept(result -> events.add("listener:A"));
        entity.thenAccept(result -> events.add("listener:B"));
        var shared = fixture.holders[sharedIndex].full;
        for (var holder : fixture.holders) {
            if (holder.full != shared) holder.full.complete(ChunkResult.of(fixture.chunk));
        }
        String before = state(entity) + "," + state(block);
        shared.complete(ChunkResult.of(fixture.chunk));
        System.out.println("overlap:" + entityFirst + ":" + sharedIndex + ":" + duplicate
            + "|" + before + "|" + state(entity) + "," + state(block)
            + "|" + String.join(";", events));
    }
    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        System.setOut(output);
        var field = Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        unsafe = (Unsafe) field.get(null);
        System.out.println("BEGIN_RESULT");
        for (var name : List.of("missing", "ready", "success", "unavailable", "exception", "cancel", "shared")) basic(name);
        for (int index : new int[] {6, 7, 8, 11, 12, 13, 16, 17, 18}) {
            for (boolean first : new boolean[] {false, true}) {
                overlap(first, index, false);
                overlap(first, index, true);
            }
        }
        System.out.println("END_RESULT");
    }
}
