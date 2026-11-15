import it.unimi.dsi.fastutil.longs.Long2ByteMap;
import it.unimi.dsi.fastutil.longs.Long2ByteMaps;
import java.util.HashMap;
import java.util.TreeSet;
import net.minecraft.SharedConstants;
import net.minecraft.core.SectionPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.DistanceManager;
import net.minecraft.server.level.LoadingChunkTracker;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.level.SimulationChunkTracker;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;
import sun.misc.Unsafe;

public class LoadingTickDependenciesOracle {
    static Unsafe unsafe;
    static int maxLevel;

    public static class Holder extends ChunkHolder {
        int ticketLevel;
        Holder() { super(null, 0, null, null, null, null); }
        @Override public int getTicketLevel() { return ticketLevel; }
    }

    public static class Manager extends DistanceManager {
        final HashMap<Long, Holder> holders = new HashMap<>();
        Manager(TicketStorage storage) { super(storage, Runnable::run, Runnable::run); }
        @Override protected boolean isChunkToRemove(long key) { return false; }
        @Override protected ChunkHolder getChunk(long key) { return holders.get(key); }
        @Override protected ChunkHolder updateChunkScheduling(long key, int level, ChunkHolder old, int previous) {
            if (level >= maxLevel) {
                holders.remove(key);
                return null;
            }
            Holder holder = (Holder) old;
            if (holder == null) {
                try { holder = (Holder) unsafe.allocateInstance(Holder.class); }
                catch (InstantiationException error) { throw new RuntimeException(error); }
                holders.put(key, holder);
            }
            holder.ticketLevel = level;
            return holder;
        }
        SimulationChunkTracker simulation() throws Exception {
            var field = DistanceManager.class.getDeclaredField("simulationChunkTracker");
            field.setAccessible(true);
            return (SimulationChunkTracker) field.get(this);
        }
        LoadingChunkTracker loading() throws Exception {
            var field = DistanceManager.class.getDeclaredField("loadingChunkTracker");
            field.setAccessible(true);
            return (LoadingChunkTracker) field.get(this);
        }
    }

    private static void run(int index, int simDistance, int viewDistance, int x, int z,
                            boolean playerActive, boolean forced, boolean savedSimulation) throws Exception {
        var storage = new TicketStorage();
        var manager = new Manager(storage);
        manager.updateSimulationDistance(simDistance);
        var player = (ServerPlayer) unsafe.allocateInstance(ServerPlayer.class);
        player.setId(1);
        if (playerActive) {
            manager.addPlayer(SectionPos.of(x, 0, z), player);
            // Fully dispatched PLAYER_LOADING tickets, before the throttling
            // and holder-future delays represented by real server tasks.
            for (int dx = -viewDistance; dx <= viewDistance; dx++) {
                for (int dz = -viewDistance; dz <= viewDistance; dz++) {
                    storage.addTicket(new Ticket(TicketType.PLAYER_LOADING, 31), new ChunkPos(x + dx, z + dz));
                }
            }
        }
        if (forced) storage.addTicket(new Ticket(TicketType.FORCED, index == 5 ? 29 : 31), new ChunkPos(40, -9));
        if (savedSimulation) storage.addTicket(new Ticket(TicketType.PLAYER_SIMULATION, 31), new ChunkPos(-40, 9));
        if (index == 4) storage.addTicket(new Ticket(TicketType.UNKNOWN, 31), new ChunkPos(-40, 9));
        manager.simulation().runAllUpdates();
        manager.loading().runDistanceUpdates(Integer.MAX_VALUE);
        var field = SimulationChunkTracker.class.getDeclaredField("chunks");
        field.setAccessible(true);
        var levels = (Long2ByteMap) field.get(manager.simulation());
        var dependencies = new TreeSet<ChunkPos>((a, b) -> {
            int comparison = Integer.compare(a.x(), b.x());
            return comparison != 0 ? comparison : Integer.compare(a.z(), b.z());
        });
        for (var entry : Long2ByteMaps.fastIterable(levels)) {
            if (entry.getByteValue() > 32 || manager.getChunkLevel(entry.getLongKey(), false) > 32) continue;
            var center = ChunkPos.unpack(entry.getLongKey());
            for (int dx = -1; dx <= 1; dx++) {
                for (int dz = -1; dz <= 1; dz++) dependencies.add(new ChunkPos(center.x() + dx, center.z() + dz));
            }
        }
        var line = new StringBuilder("DEPENDENCIES ").append(index);
        for (var position : dependencies) line.append(' ').append(position.x()).append(',').append(position.z());
        System.out.println(line);
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var unsafeField = Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        unsafe = (Unsafe) unsafeField.get(null);
        var max = LoadingChunkTracker.class.getDeclaredField("MAX_LEVEL");
        max.setAccessible(true);
        maxLevel = max.getInt(null);
        run(0, 12, 2, 0, 0, true, false, false);
        run(1, 5, 12, -4, 3, true, true, false);
        run(2, 12, 2, 0, 0, false, true, false);
        run(3, 12, 12, 0, 0, true, true, true);
        run(4, 12, 2, 0, 0, false, false, true);
        run(5, 12, 2, 0, 0, false, true, false);
    }
}
