import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.Executor;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.ChunkResult;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.entity.ChunkStatusUpdateListener;
import sun.misc.Unsafe;

/** Observe the real holder's ticket-level and ticking-future transitions. */
public class TickingHolderLifecycleOracle {
    static class Holder extends ChunkHolder {
        Holder(int level) {
            super(new ChunkPos(0, 0), level, LevelHeightAccessor.create(0, 256),
                null, (pos, old, next, set) -> set.accept(next), null);
        }
        void update(Map map) { updateFutures(map, (Executor) task -> {}); }
    }

    static class Map extends ChunkMap {
        List<CompletableFuture<ChunkResult<LevelChunk>>> ticking;
        Map() { super(null, null, null, null, null, null, null, null, null, null, 0, false); }
        @Override public CompletableFuture<ChunkResult<LevelChunk>> prepareAccessibleChunk(ChunkHolder holder) {
            return new CompletableFuture<>();
        }
        @Override public CompletableFuture<ChunkResult<LevelChunk>> prepareEntityTickingChunk(ChunkHolder holder) {
            return new CompletableFuture<>();
        }
        @Override public CompletableFuture<ChunkResult<LevelChunk>> prepareTickingChunk(ChunkHolder holder) {
            var future = new CompletableFuture<ChunkResult<LevelChunk>>();
            ticking.add(future);
            return future;
        }
    }

    static String state(CompletableFuture<ChunkResult<LevelChunk>> future) {
        if (!future.isDone()) return "waiting";
        if (future.isCompletedExceptionally()) return "exception";
        return future.join().isSuccess() ? "success" : "unavailable";
    }

    static void snapshot(String label, Holder holder, Map map) {
        String chunk;
        try { chunk = holder.getTickingChunk() == null ? "absent" : "present"; }
        catch (CompletionException error) { chunk = "exception"; }
        var saved = new ArrayList<String>();
        for (var future : map.ticking) saved.add(state(future));
        System.out.println(label + "|" + state(holder.getTickingChunkFuture()) + "|"
            + chunk + "|" + String.join(",", saved));
    }

    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        System.setOut(output);
        var field = Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (Unsafe) field.get(null);
        var map = (Map) unsafe.allocateInstance(Map.class);
        map.ticking = new ArrayList<>();
        var listener = ChunkMap.class.getDeclaredField("chunkStatusListener");
        listener.setAccessible(true);
        listener.set(map, (ChunkStatusUpdateListener) (pos, status) -> {});
        var holder = new Holder(34);
        var chunk = (LevelChunk) unsafe.allocateInstance(LevelChunk.class);

        System.out.println("BEGIN_RESULT");
        holder.update(map);
        snapshot("initial", holder, map);
        holder.setTicketLevel(32);
        snapshot("before-promotion", holder, map);
        holder.update(map);
        snapshot("promoted", holder, map);
        holder.setTicketLevel(31);
        holder.update(map);
        snapshot("entity-level", holder, map);
        map.ticking.get(0).complete(ChunkResult.of(chunk));
        snapshot("success", holder, map);
        holder.setTicketLevel(33);
        snapshot("before-demotion", holder, map);
        holder.update(map);
        snapshot("demoted-success", holder, map);
        holder.setTicketLevel(32);
        holder.update(map);
        snapshot("promoted-again", holder, map);
        holder.setTicketLevel(33);
        holder.update(map);
        snapshot("demoted-waiting", holder, map);
        holder.setTicketLevel(31);
        holder.update(map);
        snapshot("promoted-third", holder, map);
        map.ticking.get(2).completeExceptionally(new IllegalStateException("failure"));
        snapshot("exception", holder, map);
        holder.setTicketLevel(32);
        holder.update(map);
        snapshot("block-level", holder, map);
        holder.setTicketLevel(34);
        holder.update(map);
        snapshot("demoted-exception", holder, map);
        holder.setTicketLevel(32);
        holder.update(map);
        map.ticking.get(3).complete(ChunkHolder.UNLOADED_LEVEL_CHUNK);
        snapshot("unavailable", holder, map);
        holder.setTicketLevel(33);
        holder.update(map);
        snapshot("demoted-unavailable", holder, map);
        holder.setTicketLevel(32);
        holder.update(map);
        snapshot("promoted-fifth", holder, map);
        System.out.println("END_RESULT");
    }
}
