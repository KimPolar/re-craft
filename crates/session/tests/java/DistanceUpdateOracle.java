import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executor;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.ChunkLevel;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.DistanceManager;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;
import sun.misc.Unsafe;

/** Observe 26.3 distance updates with manually supplied holder operations. */
public class DistanceUpdateOracle {
    static Unsafe unsafe;
    static int unloaded;

    static String position(long key) {
        ChunkPos pos = ChunkPos.unpack(key);
        return pos.x() + "," + pos.z();
    }

    static class Holder extends ChunkHolder {
        long key;
        int level;
        List<String> events;
        Holder() { super(null, 0, null, null, null, null); }
        @Override public int getTicketLevel() { return level; }
        @Override public void updateHighestAllowedStatus(ChunkMap map) {
            events.add("H:" + position(key));
        }
        @Override public void updateFutures(ChunkMap map, Executor main) {
            events.add("F:" + position(key));
        }
    }

    static class Manager extends DistanceManager {
        final Map<Long, Holder> holders = new HashMap<>();
        final HashSet<Long> dropping = new HashSet<>();
        final List<String> events = new ArrayList<>();
        Manager(TicketStorage store) { super(store, Runnable::run, Runnable::run); }
        @Override protected boolean isChunkToRemove(long key) { return dropping.contains(key); }
        @Override protected ChunkHolder getChunk(long key) { return holders.get(key); }
        @Override protected ChunkHolder updateChunkScheduling(long key, int level, ChunkHolder old, int previous) {
            events.add("S:" + position(key) + ":" + previous + ":" + level);
            Holder holder = (Holder) old;
            if (holder != null) holder.level = level;
            if (holder != null && level >= unloaded) dropping.add(key);
            else if (holder != null) dropping.remove(key);
            if (level < unloaded && holder == null) {
                try { holder = (Holder) unsafe.allocateInstance(Holder.class); }
                catch (InstantiationException error) { throw new RuntimeException(error); }
                holder.key = key;
                holder.level = level;
                holder.events = events;
                holders.put(key, holder);
            }
            return holder;
        }
        String run(String name) {
            events.clear();
            boolean more = runAllUpdates(null);
            return "TRACE|" + name + "|" + more + "|" + String.join(";", events);
        }
    }

    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        System.setOut(output);
        var field = Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        unsafe = (Unsafe) field.get(null);
        unloaded = ChunkLevel.MAX_LEVEL + 1;
        var storage = new TicketStorage();
        var manager = new Manager(storage);
        var a = new ChunkPos(0, 0);
        var b = new ChunkPos(3, -2);
        var at = new Ticket(TicketType.UNKNOWN, unloaded - 3);
        var bt = new Ticket(TicketType.FORCED, unloaded - 2);
        var again = new Ticket(TicketType.UNKNOWN, unloaded - 2);
        var traces = new ArrayList<String>();
        storage.addTicket(at, a);
        storage.addTicket(bt, b);
        traces.add(manager.run("add"));
        traces.add(manager.run("settled"));
        storage.removeTicket(at, a);
        traces.add(manager.run("remove"));
        storage.addTicket(again, a);
        traces.add(manager.run("return"));
        storage.removeTicket(bt, b);
        traces.add(manager.run("remove_other"));
        traces.add(manager.run("settled_again"));
        System.out.println("BEGIN_RESULT");
        manager.holders.entrySet().stream().sorted(Map.Entry.comparingByKey()).forEach(entry ->
            System.out.println("ID|" + position(entry.getKey()) + "|"
                + Integer.toUnsignedString(System.identityHashCode(entry.getValue()))));
        traces.forEach(System.out::println);
        System.out.println("END_RESULT");
    }
}
