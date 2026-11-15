import it.unimi.dsi.fastutil.longs.Long2ByteMap;
import it.unimi.dsi.fastutil.longs.Long2ByteMaps;
import it.unimi.dsi.fastutil.longs.Long2ObjectMap;
import it.unimi.dsi.fastutil.longs.Long2ObjectMaps;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.SectionPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.DistanceManager;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.level.SimulationChunkTracker;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;
import sun.misc.Unsafe;

public class PlayerSimulationOracle {
    public static class Manager extends DistanceManager {
        Manager(TicketStorage storage) { super(storage, Runnable::run, Runnable::run); }
        @Override protected boolean isChunkToRemove(long key) { return false; }
        @Override protected ChunkHolder getChunk(long key) { return null; }
        @Override protected ChunkHolder updateChunkScheduling(long key, int level, ChunkHolder holder, int old) {
            return null;
        }
        SimulationChunkTracker tracker() throws Exception {
            var field = DistanceManager.class.getDeclaredField("simulationChunkTracker");
            field.setAccessible(true);
            return (SimulationChunkTracker) field.get(this);
        }
    }

    private static SectionPos previous;
    private static boolean previousIgnored;

    private static void observe(Manager manager, ServerPlayer player, SectionPos section,
                                boolean spectator, boolean spectatorsGenerateChunks) {
        boolean ignored = spectator && !spectatorsGenerateChunks;
        if (previous != null && previous.asLong() == section.asLong() && previousIgnored == ignored) return;
        if (previous != null && !previousIgnored) manager.removePlayer(previous, player);
        if (!ignored) manager.addPlayer(section, player);
        previous = section;
        previousIgnored = ignored;
    }

    private static void remove(Manager manager, ServerPlayer player) {
        if (previous != null && !previousIgnored) manager.removePlayer(previous, player);
        previous = null;
    }

    private static void snapshot(TicketStorage store, Manager manager, int round) throws Exception {
        manager.tracker().runAllUpdates();
        var activeField = TicketStorage.class.getDeclaredField("tickets");
        activeField.setAccessible(true);
        @SuppressWarnings("unchecked")
        var active = (Long2ObjectMap<List<Ticket>>) activeField.get(store);
        var ticksField = Ticket.class.getDeclaredField("ticksLeft");
        ticksField.setAccessible(true);
        var activeLine = new StringBuilder("ACTIVE ").append(round);
        for (var group : Long2ObjectMaps.fastIterable(active)) {
            var pos = ChunkPos.unpack(group.getLongKey());
            for (var ticket : group.getValue()) {
                activeLine.append(' ').append(pos.x()).append(',').append(pos.z()).append(',')
                    .append(BuiltInRegistries.TICKET_TYPE.getKey(ticket.getType())).append(',')
                    .append(ticket.getTicketLevel()).append(',').append(ticksField.getLong(ticket));
            }
        }
        System.out.println(activeLine);
        var chunksField = SimulationChunkTracker.class.getDeclaredField("chunks");
        chunksField.setAccessible(true);
        var chunks = (Long2ByteMap) chunksField.get(manager.tracker());
        var tickLine = new StringBuilder("TICK ").append(round);
        for (var entry : Long2ByteMaps.fastIterable(chunks)) {
            if (entry.getByteValue() <= 31) {
                var pos = ChunkPos.unpack(entry.getLongKey());
                tickLine.append(' ').append(pos.x()).append(',').append(pos.z()).append(',')
                    .append(entry.getByteValue());
            }
        }
        System.out.println(tickLine);
        var blockLine = new StringBuilder("BLOCK ").append(round);
        for (var entry : Long2ByteMaps.fastIterable(chunks)) {
            if (entry.getByteValue() <= 32) {
                var pos = ChunkPos.unpack(entry.getLongKey());
                blockLine.append(' ').append(pos.x()).append(',').append(pos.z()).append(',')
                    .append(entry.getByteValue());
            }
        }
        System.out.println(blockLine);
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var unsafeField = Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var player = (ServerPlayer) ((Unsafe) unsafeField.get(null)).allocateInstance(ServerPlayer.class);
        player.setId(1);
        var store = new TicketStorage();
        var manager = new Manager(store);
        manager.updateSimulationDistance(12);
        store.addTicket(new Ticket(TicketType.FORCED, 31), new ChunkPos(0, 0));
        store.addTicket(new Ticket(TicketType.PORTAL, 30), new ChunkPos(8, -2));
        snapshot(store, manager, -1);
        for (int round = 0; round < 15; round++) {
            switch (round) {
                case 0, 1 -> observe(manager, player, SectionPos.of(0, 4, 0), false, true);
                case 2 -> observe(manager, player, SectionPos.of(0, 5, 0), false, true);
                case 3 -> observe(manager, player, SectionPos.of(1, 5, 0), false, true);
                case 4 -> observe(manager, player, SectionPos.of(1, 5, 0), true, true);
                case 5 -> observe(manager, player, SectionPos.of(1, 5, 0), true, false);
                case 6 -> observe(manager, player, SectionPos.of(-3, -5, 7), true, false);
                case 7 -> observe(manager, player, SectionPos.of(-3, -5, 7), true, true);
                case 8 -> manager.updateSimulationDistance(5);
                case 9 -> observe(manager, player, SectionPos.of(-9, 3, -6), false, true);
                case 10 -> store.addTicket(new Ticket(TicketType.FORCED, 31), new ChunkPos(-9, -6));
                case 11 -> manager.updateSimulationDistance(16);
                case 12 -> observe(manager, player, SectionPos.of(-9, 3, -6), true, true);
                case 13 -> remove(manager, player);
                case 14 -> observe(manager, player, SectionPos.of(0, 4, 0), false, true);
            }
            snapshot(store, manager, round);
        }
    }
}
