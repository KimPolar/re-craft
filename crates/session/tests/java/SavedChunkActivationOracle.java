import java.lang.reflect.InvocationTargetException;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkHolder;
import net.minecraft.server.level.ChunkMap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.chunk.LevelChunk;
import sun.misc.Unsafe;

/** Observe the actual 26.3 ticking-chunk completion callback. */
public class SavedChunkActivationOracle {
    static final List<String> events = new ArrayList<>();
    static boolean failPostprocessing;

    static class TestChunk extends LevelChunk {
        TestChunk() { super((Level) null, (ChunkPos) null); }
        @Override public void postProcessGeneration(ServerLevel level) {
            events.add("postprocess");
            if (failPostprocessing) throw new IllegalStateException("postprocessing failed");
        }
    }

    static class TestLevel extends ServerLevel {
        TestLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public void startTickingChunk(LevelChunk chunk) { events.add("start-ticking"); }
    }

    static class TestHolder extends ChunkHolder {
        TestHolder() { super(null, 0, null, null, null, null); }
        @Override public CompletableFuture<?> getSendSyncFuture() {
            events.add("send-future");
            // Stop before unrelated player-send and debug synchronization.
            return null;
        }
    }

    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var unsafeField = Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (Unsafe) unsafeField.get(null);
        var map = (ChunkMap) unsafe.allocateInstance(ChunkMap.class);
        var level = (TestLevel) unsafe.allocateInstance(TestLevel.class);
        var chunk = (TestChunk) unsafe.allocateInstance(TestChunk.class);
        var holder = (TestHolder) unsafe.allocateInstance(TestHolder.class);
        var levelField = ChunkMap.class.getDeclaredField("level");
        levelField.setAccessible(true);
        levelField.set(map, level);
        var completion = ChunkMap.class.getDeclaredMethod(
                "lambda$prepareTickingChunk$2", ChunkHolder.class, List.class);
        completion.setAccessible(true);

        for (boolean fail : new boolean[] {false, true}) {
            events.clear();
            failPostprocessing = fail;
            try {
                completion.invoke(map, holder, List.of(chunk));
                throw new AssertionError("fixture did not stop");
            } catch (InvocationTargetException failure) {
                var cause = failure.getCause();
                if (fail ? !(cause instanceof IllegalStateException) : !(cause instanceof NullPointerException))
                    throw failure;
            }
            output.println((fail ? "FAILED " : "ORDER ") + String.join(" ", events));
        }
    }
}
