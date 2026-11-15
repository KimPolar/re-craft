import it.unimi.dsi.fastutil.longs.Long2ByteMap;
import it.unimi.dsi.fastutil.longs.Long2ByteMaps;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.SimulationChunkTracker;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;

public class SimulationChunksOracle {
    private static class Tracker extends SimulationChunkTracker {
        Tracker(TicketStorage storage) { super(storage); }
        int runBudget(int budget) { return runUpdates(budget); }
    }

    private static Ticket ticket(int kind, int level) {
        return new Ticket(kind == 1 ? TicketType.PLAYER_SIMULATION : TicketType.FORCED, level);
    }

    private static void add(TicketStorage storage, int x, int z, int kind, int level) {
        storage.addTicket(ticket(kind, level), new ChunkPos(x, z));
    }

    private static void remove(TicketStorage storage, int x, int z, int kind, int level) {
        storage.removeTicket(ticket(kind, level), new ChunkPos(x, z));
    }

    private static void emit(SimulationChunkTracker tracker, int step) throws Exception {
        var field = SimulationChunkTracker.class.getDeclaredField("chunks");
        field.setAccessible(true);
        var chunks = (Long2ByteMap) field.get(tracker);
        var order = new StringBuilder("TRACE ").append(step);
        for (var entry : Long2ByteMaps.fastIterable(chunks)) {
            if (entry.getByteValue() <= 31) {
                var pos = ChunkPos.unpack(entry.getLongKey());
                order.append(' ').append(pos.x()).append(':').append(pos.z());
            }
        }
        System.out.println(order);
        var levels = new StringBuilder("LEVEL ").append(step).append(' ');
        for (int z = -18; z <= 18; z++) for (int x = -18; x <= 24; x++) {
            levels.append((char) ('A' + tracker.getLevel(new ChunkPos(x, z))));
        }
        System.out.println(levels);
    }

    private static void trace(SimulationChunkTracker tracker, int step) throws Exception {
        tracker.runAllUpdates();
        emit(tracker, step);
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var storage = new TicketStorage();
        var tracker = new Tracker(storage);
        trace(tracker, 0);
        add(storage, 0, 0, 1, 26);
        add(storage, 0, 0, 1, 26);
        trace(tracker, 1);
        add(storage, 8, 1, 2, 31);
        add(storage, -7, -4, 1, 27);
        trace(tracker, 2);
        remove(storage, 0, 0, 1, 26);
        add(storage, 1, -1, 1, 26);
        trace(tracker, 3);
        add(storage, 1, -1, 2, 30);
        add(storage, 1, -1, 1, 28);
        remove(storage, 1, -1, 1, 26);
        trace(tracker, 4);
        remove(storage, -7, -4, 1, 27);
        remove(storage, 8, 1, 2, 31);
        remove(storage, 1, -1, 2, 30);
        trace(tracker, 5);
        remove(storage, 1, -1, 1, 28);
        add(storage, 13, -12, 1, 19);
        trace(tracker, 6);
        remove(storage, 13, -12, 1, 19);
        trace(tracker, 7);
        for (int round = 0; round < 20; round++) {
            if (round > 0) {
                int last = round - 1;
                remove(storage, (last * 7) % 21 - 10, (last * 11) % 19 - 9,
                       1, 31 - (5 + (last * 13) % 8));
            }
            if (round >= 3 && (round - 3) % 4 == 0) {
                int old = round - 3;
                remove(storage, (old * 7) % 21 - 8, (old * 11) % 19 - 12, 2, 31);
            }
            int x = (round * 7) % 21 - 10;
            int z = (round * 11) % 19 - 9;
            add(storage, x, z, 1, 31 - (5 + (round * 13) % 8));
            if (round % 4 == 0) add(storage, x + 2, z - 3, 2, 31);
            trace(tracker, 8 + round);
        }
        add(storage, -2, 3, 2, 18);
        int[] budgets = {0, 1, 2, 7, 31, 111, 100000};
        for (int index = 0; index < budgets.length; index++) {
            int remaining = tracker.runBudget(budgets[index]);
            System.out.println("BUDGET " + index + " " + remaining + " " + (tracker.getQueueSize() > 0));
            emit(tracker, 28 + index);
        }
    }
}
