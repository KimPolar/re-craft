import it.unimi.dsi.fastutil.longs.Long2ByteMap;
import it.unimi.dsi.fastutil.longs.Long2ByteMaps;
import it.unimi.dsi.fastutil.longs.Long2ObjectMap;
import it.unimi.dsi.fastutil.longs.Long2ObjectMaps;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.IntTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.SimulationChunkTracker;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;

public class SavedSimulationTicketsOracle {
    private static CompoundTag extra(int x, int z) {
        var entry = new CompoundTag();
        entry.putIntArray("chunk_pos", new int[]{x, z});
        entry.putString("type", "minecraft:forced");
        entry.putInt("level", 31);
        return entry;
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var storage = new TicketStorage();
        for (int i = 0; i < 90; i++) {
            var pos = new ChunkPos((i * 37) % 131 - 65, (i * 61) % 127 - 63);
            var type = i % 2 == 0 ? TicketType.FORCED : TicketType.PORTAL;
            storage.addTicket(new Ticket(type, 28 + i % 4), pos);
            if (i % 9 == 0) storage.addTicket(new Ticket(TicketType.FORCED, 31), pos);
        }
        storage.addTicket(new Ticket(TicketType.FORCED, 31), new ChunkPos(0, 0));
        storage.addTicket(new Ticket(TicketType.PLAYER_SIMULATION, 20), new ChunkPos(0, 0));
        var data = (CompoundTag) TicketStorage.CODEC.encodeStart(NbtOps.INSTANCE, storage).getOrThrow();
        var list = (ListTag) data.get("tickets");
        list.add(list.get(0).copy());
        var bytes = extra(-3, 4);
        bytes.putByteArray("chunk_pos", new byte[]{-3, 4});
        list.add(bytes);
        var longs = extra(-7, 9);
        longs.putLongArray("chunk_pos", new long[]{-7, 9});
        list.add(longs);
        var integers = extra(11, -12);
        var posList = new ListTag();
        posList.add(IntTag.valueOf(11));
        posList.add(IntTag.valueOf(-12));
        integers.put("chunk_pos", posList);
        list.add(integers);
        data.putString("future_field", "retained");
        var root = new CompoundTag();
        root.putInt("DataVersion", 5023);
        root.put("data", data);
        NbtIo.write(root, java.nio.file.Path.of(args[0]));

        var decoded = TicketStorage.CODEC.parse(NbtOps.INSTANCE, data).getOrThrow();
        var deactivatedField = TicketStorage.class.getDeclaredField("deactivatedTickets");
        deactivatedField.setAccessible(true);
        @SuppressWarnings("unchecked")
        var deactivated = (Long2ObjectMap<java.util.List<Ticket>>) deactivatedField.get(decoded);
        var ticksField = Ticket.class.getDeclaredField("ticksLeft");
        ticksField.setAccessible(true);
        var activation = new StringBuilder("ACT");
        for (var group : Long2ObjectMaps.fastIterable(deactivated)) {
            var pos = ChunkPos.unpack(group.getLongKey());
            for (var ticket : group.getValue()) {
                activation.append(' ').append(pos.x()).append(',').append(pos.z()).append(',')
                    .append(BuiltInRegistries.TICKET_TYPE.getKey(ticket.getType())).append(',')
                    .append(ticket.getTicketLevel()).append(',').append(ticksField.getLong(ticket));
            }
        }
        System.out.println(activation);
        var tracker = new SimulationChunkTracker(decoded);
        decoded.activateAllDeactivatedTickets();
        decoded.addTicket(new Ticket(TicketType.PLAYER_SIMULATION, 24), new ChunkPos(2, -5));
        tracker.runAllUpdates();
        var chunksField = SimulationChunkTracker.class.getDeclaredField("chunks");
        chunksField.setAccessible(true);
        var chunks = (Long2ByteMap) chunksField.get(tracker);
        var order = new StringBuilder("TICK");
        for (var entry : Long2ByteMaps.fastIterable(chunks)) {
            if (entry.getByteValue() <= 31) {
                var pos = ChunkPos.unpack(entry.getLongKey());
                order.append(' ').append(pos.x()).append(',').append(pos.z()).append(',')
                    .append(entry.getByteValue());
            }
        }
        System.out.println(order);
    }
}
