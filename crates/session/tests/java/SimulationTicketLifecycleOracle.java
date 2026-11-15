import it.unimi.dsi.fastutil.HashCommon;
import it.unimi.dsi.fastutil.longs.Long2ByteMap;
import it.unimi.dsi.fastutil.longs.Long2ByteMaps;
import it.unimi.dsi.fastutil.longs.Long2ObjectMap;
import it.unimi.dsi.fastutil.longs.Long2ObjectMaps;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.SimulationChunkTracker;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;
import sun.misc.Unsafe;

public class SimulationTicketLifecycleOracle {
    public static class FakeHolder extends ChunkHolder {
        boolean ready;
        FakeHolder() { super(null, 0, null, null, null, null); }
        @Override public boolean isReadyForSaving() { return ready; }
    }

    public static class FakeMap extends ChunkMap {
        int round;
        FakeHolder pending;
        FakeHolder ready;
        FakeMap() { super(null, null, null, null, null, null, null, null, null, null, 0, false); }
        @Override public ChunkHolder getUpdatingChunkIfPresent(long key) {
            var pos = ChunkPos.unpack(key);
            return switch (Math.floorMod(pos.x() * 17 + pos.z() * 31 + round, 3)) {
                case 0 -> pending;
                case 1 -> ready;
                default -> null;
            };
        }
    }

    private static CompoundTag ticket(int x, int z, String type, int level, long ticks) {
        var entry = new CompoundTag();
        entry.putIntArray("chunk_pos", new int[] {x, z});
        entry.putString("type", "minecraft:" + type);
        entry.putInt("level", level);
        entry.putLong("ticks_left", ticks);
        return entry;
    }

    private static void snapshot(TicketStorage store, SimulationChunkTracker tracker, int round)
            throws Exception {
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
        var chunks = (Long2ByteMap) chunksField.get(tracker);
        var tickLine = new StringBuilder("TICK ").append(round);
        for (var entry : Long2ByteMaps.fastIterable(chunks)) {
            if (entry.getByteValue() <= 31) {
                var pos = ChunkPos.unpack(entry.getLongKey());
                tickLine.append(' ').append(pos.x()).append(',').append(pos.z()).append(',')
                    .append(entry.getByteValue());
            }
        }
        System.out.println(tickLine);
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var list = new ListTag();
        var types = new String[] {"forced", "portal", "ender_pearl", "unknown", "spawn_search",
            "player_simulation", "dragon", "player_spawn"};
        for (int i = 0; i < 48; i++) {
            var entry = ticket((i * 37) % 97 - 48, (i * 61) % 101 - 50,
                types[i % types.length], 29 + i % 4, i % 5 - 1);
            list.add(entry);
            if (i % 13 == 0) list.add(entry.copy());
        }
        int collisions = 0;
        for (int x = 1; collisions < 32; x++) {
            if ((HashCommon.mix(ChunkPos.pack(x, 0)) & 127) == 127) {
                list.add(ticket(x, 0, collisions % 5 == 0 ? "forced" : "portal",
                    31, collisions % 3));
                collisions++;
            }
        }
        list.add(ticket(0, 0, "forced", 31, 0));
        list.add(ticket(0, 0, "portal", 29, 0));
        list.add(ticket(0, 0, "ender_pearl", 30, 0));
        list.add(ticket(0, 0, "unknown", 31, 0));
        list.add(ticket(3, -8, "portal", 29, Long.MIN_VALUE));
        var data = new CompoundTag();
        data.put("tickets", list);
        var root = new CompoundTag();
        root.putInt("DataVersion", 5023);
        root.put("data", data);
        NbtIo.write(root, java.nio.file.Path.of(args[0]));

        var store = TicketStorage.CODEC.parse(NbtOps.INSTANCE, data).getOrThrow();
        var tracker = new SimulationChunkTracker(store);
        store.activateAllDeactivatedTickets();
        var unsafeField = Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (Unsafe) unsafeField.get(null);
        var map = (FakeMap) unsafe.allocateInstance(FakeMap.class);
        map.pending = (FakeHolder) unsafe.allocateInstance(FakeHolder.class);
        map.ready = (FakeHolder) unsafe.allocateInstance(FakeHolder.class);
        map.ready.ready = true;
        tracker.runAllUpdates();
        snapshot(store, tracker, -1);
        var portalPos = new ChunkPos(3, -8);
        var zero = new ChunkPos(0, 0);
        for (int round = 0; round < 12; round++) {
            if (round == 2 || round == 4) store.addTicket(new Ticket(TicketType.PORTAL, 29), portalPos);
            if (round == 3) store.replaceTicketLevelOfType(27, TicketType.PLAYER_SIMULATION);
            if (round == 5) store.removeTicket(new Ticket(TicketType.FORCED, 31), zero);
            map.round = round;
            store.purgeStaleTickets(map);
            tracker.runAllUpdates();
            snapshot(store, tracker, round);
        }
    }
}
