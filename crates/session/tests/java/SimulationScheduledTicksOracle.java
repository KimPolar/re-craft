import java.util.ArrayList;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.SectionPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.DistanceManager;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.level.SimulationChunkTracker;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;
import net.minecraft.world.ticks.LevelChunkTicks;
import net.minecraft.world.ticks.LevelTicks;
import net.minecraft.world.ticks.SavedTick;
import net.minecraft.world.ticks.TickPriority;
import sun.misc.Unsafe;

public class SimulationScheduledTicksOracle {
    static class Manager extends DistanceManager {
        Manager(TicketStorage storage) { super(storage, Runnable::run, Runnable::run); }
        @Override protected boolean isChunkToRemove(long pos) { return false; }
        @Override protected ChunkHolder getChunk(long pos) { return null; }
        @Override protected ChunkHolder updateChunkScheduling(long pos, int level, ChunkHolder holder, int old) {
            return null;
        }
        void updateSimulation() throws Exception {
            var field = DistanceManager.class.getDeclaredField("simulationChunkTracker");
            field.setAccessible(true);
            ((SimulationChunkTracker) field.get(this)).runAllUpdates();
        }
    }

    private static LevelTicks<String> prepare(Manager manager, String type) {
        var level = new LevelTicks<String>(manager::inBlockTickingRange);
        for (int x = -2; x <= 2; x++) {
            var pending = new SavedTick<>(type, new BlockPos(x * 16 + 4, 2, 4), 0, TickPriority.NORMAL);
            var chunk = new LevelChunkTicks<String>(List.of(pending));
            level.addContainer(new ChunkPos(x, 0), chunk);
            chunk.unpack(42);
        }
        return level;
    }

    private static void tick(LevelTicks<String> ticks, String name, int step, long time) {
        var order = new ArrayList<Integer>();
        ticks.tick(time, 65536, (position, type) -> order.add(Math.floorDiv(position.getX(), 16)));
        var line = new StringBuilder(name).append(' ').append(step);
        for (int x : order) line.append(' ').append(x);
        System.out.println(line);
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var player = (ServerPlayer) ((Unsafe) field.get(null)).allocateInstance(ServerPlayer.class);
        player.setId(1);
        var storage = new TicketStorage();
        var manager = new Manager(storage);
        manager.updateSimulationDistance(0);
        var previous = SectionPos.of(0, 4, 0);
        manager.addPlayer(previous, player);
        manager.updateSimulation();
        var blocks = prepare(manager, "minecraft:sand");
        var fluids = prepare(manager, "minecraft:water");
        tick(blocks, "BLOCK", 0, 42);
        tick(fluids, "FLUID", 0, 42);
        manager.removePlayer(previous, player);
        previous = SectionPos.of(1, 4, 0);
        manager.addPlayer(previous, player);
        manager.updateSimulation();
        tick(blocks, "BLOCK", 1, 43);
        tick(fluids, "FLUID", 1, 43);
        manager.removePlayer(previous, player);
        storage.addTicket(new Ticket(TicketType.FORCED, 31), new ChunkPos(-2, 0));
        manager.updateSimulation();
        tick(blocks, "BLOCK", 2, 44);
        tick(fluids, "FLUID", 2, 44);
    }
}
