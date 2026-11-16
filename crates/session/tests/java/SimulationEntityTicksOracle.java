import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.SectionPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.DistanceManager;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.level.SimulationChunkTracker;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;
import sun.misc.Unsafe;

public class SimulationEntityTicksOracle {
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

    static class World extends ServerLevel {
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public int getNextEntityId() { return 1; }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (Unsafe) field.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        var player = (ServerPlayer) unsafe.allocateInstance(ServerPlayer.class);
        player.setId(1);
        var entities = new Entity[] {
            new ItemEntity(EntityTypes.ITEM, world),
            new ExperienceOrb(EntityTypes.EXPERIENCE_ORB, world),
            new FallingBlockEntity(EntityTypes.FALLING_BLOCK, world),
        };
        var storage = new TicketStorage();
        var manager = new Manager(storage);
        manager.updateSimulationDistance(5);
        var section = SectionPos.of(0, 4, 0);
        manager.addPlayer(section, player);
        double[] xs = {-96.001, -96.0, -95.999, -16.001, -16.0, -0.0001, 0.0, 15.999, 16.0, 80.0, 96.0};
        double[] zs = {-96.0, -16.001, -16.0, -0.0001, 0.0, 15.999, 16.0};
        for (int round = 0; round < 4; round++) {
            if (round == 1) manager.updateSimulationDistance(6);
            if (round == 2) {
                manager.removePlayer(section, player);
                section = SectionPos.of(-1, 4, 1);
                manager.addPlayer(section, player);
            }
            if (round == 3) {
                manager.removePlayer(section, player);
                storage.addTicket(new Ticket(TicketType.FORCED, 31), new ChunkPos(-6, 0));
            }
            manager.updateSimulation();
            for (int type = 0; type < entities.length; type++) {
                for (int x = 0; x < xs.length; x++) {
                    for (int z = 0; z < zs.length; z++) {
                        var entity = entities[type];
                        entity.setPosRaw(xs[x], 64.0, zs[z]);
                        var chunk = entity.chunkPosition();
                        System.out.println("ENTITY " + round + " " + type + " " + x + " " + z + " "
                            + chunk.x() + " " + chunk.z() + " " + manager.inEntityTickingRange(chunk.pack()));
                    }
                }
            }
        }
    }
}
