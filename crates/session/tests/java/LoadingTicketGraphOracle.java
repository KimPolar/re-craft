import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.ChunkLevel;
import net.minecraft.server.level.DistanceManager;
import net.minecraft.server.level.LoadingChunkTracker;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;
import sun.misc.Unsafe;

/** Record the real loading distance graph's scheduling calls and budgets. */
public class LoadingTicketGraphOracle {
    static Unsafe unsafe;
    static int unloaded;

    static class Holder extends ChunkHolder {
        int level;
        Holder() { super(null, 0, null, null, null, null); }
        @Override public int getTicketLevel() { return level; }
    }

    static class Manager extends DistanceManager {
        final HashMap<Long, Holder> holders = new HashMap<>();
        final HashSet<Long> dropping = new HashSet<>();
        final List<String> changes = new ArrayList<>();
        Manager(TicketStorage storage) { super(storage, Runnable::run, Runnable::run); }
        @Override protected boolean isChunkToRemove(long key) { return dropping.contains(key); }
        @Override protected ChunkHolder getChunk(long key) { return holders.get(key); }
        @Override protected ChunkHolder updateChunkScheduling(long key, int level, ChunkHolder previous, int old) {
            var pos = ChunkPos.unpack(key);
            changes.add(pos.x() + "," + pos.z() + "," + old + "," + level);
            var holder = (Holder) previous;
            if (holder != null) holder.level = level;
            if (holder != null && level >= unloaded) dropping.add(key);
            else if (holder != null) dropping.remove(key);
            if (level < unloaded && holder == null) {
                try { holder = (Holder) unsafe.allocateInstance(Holder.class); }
                catch (InstantiationException error) { throw new RuntimeException(error); }
                holder.level = level;
                holders.put(key, holder);
            }
            return holder;
        }
        LoadingChunkTracker loading() throws Exception {
            var field = DistanceManager.class.getDeclaredField("loadingChunkTracker");
            field.setAccessible(true);
            return (LoadingChunkTracker) field.get(this);
        }
        void step(String name, int budget) throws Exception {
            changes.clear();
            int remaining = loading().runDistanceUpdates(budget);
            System.out.println("RESULT|" + name + "|" + remaining + "|" + String.join(";", changes));
        }
    }

    static void add(TicketStorage store, TicketType type, int level, int x, int z) {
        store.addTicket(new Ticket(type, level), new ChunkPos(x, z));
    }
    static void remove(TicketStorage store, TicketType type, int level, int x, int z) {
        store.removeTicket(new Ticket(type, level), new ChunkPos(x, z));
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
        System.out.println("MAX|" + unloaded);

        var store = new TicketStorage();
        var manager = new Manager(store);
        add(store, TicketType.PLAYER_LOADING, 31, 0, 0);
        add(store, TicketType.UNKNOWN, 33, 0, 0);
        add(store, TicketType.FORCED, 33, 0, 0);
        add(store, TicketType.FORCED, 30, 2, -2);
        add(store, TicketType.PLAYER_SIMULATION, 19, 4, 1);
        add(store, TicketType.UNKNOWN, 31, -2, 3);
        add(store, TicketType.UNKNOWN, 33, -2, 3);
        add(store, TicketType.PLAYER_LOADING, 31, 0, 0);
        manager.step("a0", 0);
        manager.step("a1", 1);
        manager.step("a2", 7);
        add(store, TicketType.UNKNOWN, 29, 1, 1);
        remove(store, TicketType.UNKNOWN, 29, 1, 1);
        manager.step("a3", 37);
        manager.step("a4", 4096);

        remove(store, TicketType.UNKNOWN, 33, 0, 0);
        remove(store, TicketType.PLAYER_LOADING, 31, 0, 0);
        manager.step("b0", 13);
        manager.step("b1", 4096);

        add(store, TicketType.UNKNOWN, 29, 1, 1);
        remove(store, TicketType.FORCED, 30, 2, -2);
        manager.step("c0", 5);
        manager.step("c1", 13);
        manager.step("c2", 4096);

        add(store, TicketType.PLAYER_LOADING, 32, 1, 1);
        remove(store, TicketType.UNKNOWN, 29, 1, 1);
        manager.step("d0", 15);
        manager.step("d1", 4096);

        store.purgeStaleTickets(null);
        store.purgeStaleTickets(null);
        manager.step("e0", 7);
        manager.step("e1", 4096);
    }
}
