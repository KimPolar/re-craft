import it.unimi.dsi.fastutil.longs.Long2IntMap;
import it.unimi.dsi.fastutil.longs.Long2ObjectLinkedOpenHashMap;
import it.unimi.dsi.fastutil.longs.LongSet;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashSet;
import java.util.List;
import java.util.concurrent.Executor;
import net.minecraft.SharedConstants;
import net.minecraft.core.SectionPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.ChunkTaskDispatcher;
import net.minecraft.server.level.ChunkTaskPriorityQueue;
import net.minecraft.server.level.DistanceManager;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.level.ThrottlingChunkTaskDispatcher;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;
import sun.misc.Unsafe;

/** Observe the actual player-ticket tracker with manually stepped executors. */
public class PlayerLoadingOracle {
    static final Comparator<Long> POSITIONS = (a, b) -> {
        var left = ChunkPos.unpack(a);
        var right = ChunkPos.unpack(b);
        int x = Integer.compare(left.x(), right.x());
        return x != 0 ? x : Integer.compare(left.z(), right.z());
    };
    static String pos(long packed) {
        var value = ChunkPos.unpack(packed);
        return value.x() + "," + value.z();
    }
    static Object field(Object owner, Class<?> type, String name) throws Exception {
        Field field = type.getDeclaredField(name);
        field.setAccessible(true);
        return field.get(owner);
    }

    static class Manual implements Executor {
        final ArrayDeque<Runnable> pending = new ArrayDeque<>();
        @Override public void execute(Runnable task) { pending.addLast(task); }
        boolean one() {
            var task = pending.pollFirst();
            if (task == null) return false;
            task.run();
            return true;
        }
        void drain() {
            int count = 0;
            while (one()) if (++count > 100_000) throw new IllegalStateException("executor did not settle");
        }
    }

    static class Store extends TicketStorage {
        final List<String> events = new ArrayList<>();
        final HashSet<Long> active = new HashSet<>();
        @Override public boolean addTicket(long pos, Ticket ticket) {
            boolean changed = super.addTicket(pos, ticket);
            if (ticket.getType() == TicketType.PLAYER_LOADING) {
                events.add("+" + pos(pos) + ":" + changed);
                if (changed) active.add(pos);
            }
            return changed;
        }
        @Override public boolean removeTicket(long pos, Ticket ticket) {
            boolean changed = super.removeTicket(pos, ticket);
            if (ticket.getType() == TicketType.PLAYER_LOADING) {
                events.add("-" + pos(pos) + ":" + changed);
                if (changed) active.remove(pos);
            }
            return changed;
        }
    }

    static class Manager extends DistanceManager {
        Manager(Store store, Manual worker, Manual main) { super(store, worker, main); }
        @Override protected boolean isChunkToRemove(long key) { return false; }
        @Override protected ChunkHolder getChunk(long key) { return null; }
        @Override protected ChunkHolder updateChunkScheduling(long key, int level, ChunkHolder old, int previous) {
            return null;
        }
        void view(int distance) { updatePlayerTickets(distance); }
        Object tracker() throws Exception { return field(this, DistanceManager.class, "playerTicketManager"); }
        void updateTracker() throws Exception {
            var tracker = tracker();
            Method method = tracker.getClass().getDeclaredMethod("runAllUpdates");
            method.setAccessible(true);
            method.invoke(tracker);
        }
        ThrottlingChunkTaskDispatcher dispatcher() throws Exception {
            return (ThrottlingChunkTaskDispatcher) field(this, DistanceManager.class, "ticketDispatcher");
        }
        LongSet releases() throws Exception { return (LongSet) field(this, DistanceManager.class, "ticketsToRelease"); }
    }

    static void pump(Manual worker, Manual main, int limit) {
        for (int i = 0; i < limit; i++) {
            worker.drain();
            if (!main.one()) break;
        }
        worker.drain();
    }
    static String positions(Iterable<Long> values, boolean sorted) {
        var result = new ArrayList<Long>();
        for (var value : values) result.add(value);
        if (sorted) result.sort(POSITIONS);
        var text = new ArrayList<String>();
        for (long value : result) text.add(pos(value));
        return String.join(";", text);
    }
    static void snapshot(String label, Manager manager, Store store, Manual worker, Manual main) throws Exception {
        var dispatcher = manager.dispatcher();
        var acquired = (LongSet) field(dispatcher, ThrottlingChunkTaskDispatcher.class, "chunkPositionsInExecution");
        var queue = (ChunkTaskPriorityQueue) field(dispatcher, ChunkTaskDispatcher.class, "queue");
        var buckets = (List<?>) field(queue, ChunkTaskPriorityQueue.class, "queuesPerPriority");
        var queued = new ArrayList<String>();
        for (int i = 0; i < buckets.size(); i++) {
            var bucket = (Long2ObjectLinkedOpenHashMap<?>) buckets.get(i);
            for (var entry : bucket.long2ObjectEntrySet())
                queued.add(i + ":" + pos(entry.getLongKey()) + ":" + ((List<?>) entry.getValue()).size());
        }
        var tracker = manager.tracker();
        var levels = (Long2IntMap) field(tracker, tracker.getClass(), "queueLevels");
        var samples = new ArrayList<String>();
        for (int x = -3; x <= 3; x++) for (int z = -2; z <= 2; z++) {
            long key = new ChunkPos(x, z).pack();
            int value = levels.get(key);
            if (value != levels.defaultReturnValue()) samples.add(x + "," + z + ":" + value);
        }
        System.out.println("SNAP|" + label + "|" + String.join(";", store.events)
            + "|" + positions(store.active, true) + "|" + positions(acquired, true)
            + "|" + String.join(";", queued) + "|" + positions(manager.releases(), false)
            + "|" + worker.pending.size() + "," + main.pending.size() + "|" + String.join(";", samples));
        store.events.clear();
    }

    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        System.setOut(output);
        var unsafeField = Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (Unsafe) unsafeField.get(null);
        var player = (ServerPlayer) unsafe.allocateInstance(ServerPlayer.class);
        player.setId(1);
        var worker = new Manual();
        var main = new Manual();
        var store = new Store();
        var manager = new Manager(store, worker, main);
        System.out.println("BEGIN_RESULT");
        manager.view(2);
        manager.addPlayer(SectionPos.of(0, 0, 0), player);
        manager.updateTracker();
        snapshot("queued", manager, store, worker, main);
        pump(worker, main, 20);
        snapshot("four", manager, store, worker, main);

        manager.releases().clear();
        main.execute(() -> {
            try { manager.dispatcher().release(new ChunkPos(0, 0).pack(), () -> {}, false); }
            catch (Exception error) { throw new RuntimeException(error); }
        });
        pump(worker, main, 10);
        snapshot("release", manager, store, worker, main);
        manager.removePlayer(SectionPos.of(0, 0, 0), player);
        manager.addPlayer(SectionPos.of(0, 1, 0), player);
        manager.updateTracker();
        pump(worker, main, 10);
        snapshot("vertical", manager, store, worker, main);

        manager.removePlayer(SectionPos.of(0, 0, 0), player);
        manager.addPlayer(SectionPos.of(1, 0, 0), player);
        manager.updateTracker();
        pump(worker, main, 12);
        snapshot("move", manager, store, worker, main);
        manager.view(1);
        pump(worker, main, 20);
        snapshot("view", manager, store, worker, main);
        manager.removePlayer(SectionPos.of(1, 0, 0), player);
        manager.updateTracker();
        pump(worker, main, 80);
        snapshot("removed", manager, store, worker, main);
        manager.view(2);
        manager.addPlayer(SectionPos.of(-2, 0, 1), player);
        manager.updateTracker();
        pump(worker, main, 20);
        snapshot("return", manager, store, worker, main);
        main.execute(() -> {
            try { manager.dispatcher().release(new ChunkPos(-1, 1).pack(), () -> {}, false); }
            catch (Exception error) { throw new RuntimeException(error); }
        });
        pump(worker, main, 10);
        snapshot("late-old", manager, store, worker, main);

        var worker2 = new Manual();
        var main2 = new Manual();
        var store2 = new Store();
        var manager2 = new Manager(store2, worker2, main2);
        manager2.view(2);
        manager2.addPlayer(SectionPos.of(0, 0, 0), player);
        manager2.updateTracker();
        pump(worker2, main2, 1);
        snapshot("race-inflight", manager2, store2, worker2, main2);
        manager2.removePlayer(SectionPos.of(0, 0, 0), player);
        manager2.addPlayer(SectionPos.of(20, 0, 0), player);
        manager2.updateTracker();
        pump(worker2, main2, 40);
        snapshot("race-moved", manager2, store2, worker2, main2);
        System.out.println("END_RESULT");
    }
}
