import it.unimi.dsi.fastutil.longs.Long2ObjectLinkedOpenHashMap;
import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Executor;
import net.minecraft.SharedConstants;
import net.minecraft.core.SectionPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.ChunkResult;
import net.minecraft.server.level.DistanceManager;
import net.minecraft.server.level.GenerationChunkHolder;
import net.minecraft.server.level.PlayerMap;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.util.Util;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.TicketStorage;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;
import sun.misc.Unsafe;

public class SpawnChunkOracle {
    private static Unsafe unsafe;

    static class Player extends ServerPlayer {
        Vec3 testPosition;
        boolean testSpectator;
        Player() { super(null, null, null, null); }
        @Override public Vec3 position() { return testPosition; }
        @Override public boolean isSpectator() { return testSpectator; }
    }

    private static void field(Class<?> owner, Object object, String name, Object value) throws Exception {
        Field field = owner.getDeclaredField(name);
        field.setAccessible(true);
        field.set(object, value);
    }

    private static int state(ChunkPos position, int index) {
        return Math.floorMod(position.x() * 31 + position.z() * 17 + index, 7);
    }

    private static void run(int index, int x, int z, double px, double pz,
                            boolean spectator, boolean spectatorLoads, long seed) throws Exception {
        var map = (ChunkMap) unsafe.allocateInstance(ChunkMap.class);
        var player = (Player) unsafe.allocateInstance(Player.class);
        player.setId(1);
        player.testPosition = new Vec3(px, 64, pz);
        player.testSpectator = spectator;
        var players = new PlayerMap();
        players.addPlayer(player, spectator && !spectatorLoads);
        field(ChunkMap.class, map, "playerMap", players);
        var visible = new Long2ObjectLinkedOpenHashMap<ChunkHolder>();
        field(ChunkMap.class, map, "visibleChunkMap", visible);
        var managerClass = Class.forName("net.minecraft.server.level.ChunkMap$DistanceManager");
        var constructor = managerClass.getDeclaredConstructor(
            ChunkMap.class, TicketStorage.class, Executor.class, Executor.class);
        constructor.setAccessible(true);
        var manager = (DistanceManager) constructor.newInstance(
            map, new TicketStorage(), (Executor) Runnable::run, (Executor) Runnable::run);
        field(ChunkMap.class, map, "distanceManager", manager);
        if (!spectator || spectatorLoads) manager.addPlayer(SectionPos.of(x, 4, z), player);
        var candidates = manager.getSpawnCandidateChunks();
        while (candidates.hasNext()) {
            long key = candidates.nextLong();
            var position = ChunkPos.unpack(key);
            int readiness = state(position, index);
            if (readiness == 0) continue;
            var holder = (ChunkHolder) unsafe.allocateInstance(ChunkHolder.class);
            field(GenerationChunkHolder.class, holder, "pos", position);
            CompletableFuture<ChunkResult<LevelChunk>> future;
            if (readiness == 1) {
                future = new CompletableFuture<>();
            } else if (readiness == 2) {
                future = CompletableFuture.completedFuture(ChunkHolder.UNLOADED_LEVEL_CHUNK);
            } else {
                var chunk = (LevelChunk) unsafe.allocateInstance(LevelChunk.class);
                field(ChunkAccess.class, chunk, "chunkPos", position);
                future = CompletableFuture.completedFuture(ChunkResult.of(chunk));
            }
            field(ChunkHolder.class, holder, "tickingChunkFuture", future);
            visible.put(key, holder);
        }
        var chunks = new ArrayList<LevelChunk>();
        map.collectSpawningChunks(chunks);
        var collected = new StringBuilder("COLLECT ").append(index);
        for (var chunk : chunks) {
            var pos = chunk.getPos();
            collected.append(' ').append(pos.x()).append(',').append(pos.z());
        }
        System.out.println(collected);
        var random = new LegacyRandomSource(seed);
        Util.shuffle(chunks, random);
        var shuffled = new StringBuilder("SHUFFLE ").append(index);
        for (var chunk : chunks) {
            var pos = chunk.getPos();
            shuffled.append(' ').append(pos.x()).append(',').append(pos.z());
        }
        shuffled.append(" next=").append(random.nextInt());
        System.out.println(shuffled);
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        unsafe = (Unsafe) field.get(null);
        run(0, 0, 0, 8.0, 8.0, false, true, 0);
        run(1, -1, -1, -0.5, -15.5, false, true, 123456789);
        run(2, 3, -4, 63.999, -63.999, false, true, -1);
        run(3, 0, 0, 8.0, 8.0, true, true, 42);
        run(4, 0, 0, 8.0, 8.0, true, false, 42);
        run(5, 1875000, -1875000, 30000008.0, -29999992.0, false, true, 1);
    }
}
