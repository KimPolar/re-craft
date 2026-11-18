import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.util.concurrent.CompletableFuture;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ChunkResult;
import net.minecraft.server.level.GenerationChunkHolder;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import sun.misc.Unsafe;

/** Observe a real 26.3 holder's FULL future across loading-level changes. */
public class FullGenerationHolderOracle {
    static Method request;
    static Method complete;
    static ChunkAccess chunk;

    static class Holder extends GenerationChunkHolder {
        int level;

        Holder(int level) {
            super(new ChunkPos(0, 0));
            this.level = level;
        }

        @Override public int getTicketLevel() { return level; }
        @Override public int getQueueLevel() { return level; }
        @Override protected void addSaveDependency(CompletableFuture<?> future) {}
        void update() { updateHighestAllowedStatus(null); }

        @SuppressWarnings("unchecked")
        CompletableFuture<ChunkResult<ChunkAccess>> full() throws Exception {
            return (CompletableFuture<ChunkResult<ChunkAccess>>) request.invoke(this, ChunkStatus.FULL);
        }

        void finish() throws Exception {
            complete.invoke(this, ChunkStatus.FULL, chunk);
        }
    }

    static String state(CompletableFuture<ChunkResult<ChunkAccess>> future) {
        if (!future.isDone()) return "waiting";
        if (future.isCompletedExceptionally()) return "exception";
        return future.join().isSuccess() ? "full" : "unavailable";
    }

    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        System.setOut(output);
        request = GenerationChunkHolder.class.getDeclaredMethod("getOrCreateFuture", ChunkStatus.class);
        request.setAccessible(true);
        complete = GenerationChunkHolder.class.getDeclaredMethod("completeFuture", ChunkStatus.class, ChunkAccess.class);
        complete.setAccessible(true);
        var field = Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        chunk = (LevelChunk) ((Unsafe) field.get(null)).allocateInstance(LevelChunk.class);

        var holder = new Holder(32);
        var initial = holder.full();
        holder.update();
        var first = holder.full();
        var again = holder.full();
        holder.level = 34;
        var beforeDemotion = holder.full();
        holder.update();
        var unloaded = holder.full();
        holder.level = 33;
        var beforePromotion = holder.full();
        holder.update();
        var second = holder.full();
        var secondBeforeCompletion = state(second);
        holder.finish();
        var completed = holder.full();
        holder.level = 34;
        holder.update();
        var hidden = holder.full();
        holder.level = 31;
        holder.update();
        var restored = holder.full();
        boolean duplicateRejected = false;
        try { holder.finish(); }
        catch (InvocationTargetException error) {
            duplicateRejected = error.getCause() instanceof IllegalStateException;
        }

        var late = new Holder(32);
        late.update();
        var lateOld = late.full();
        late.level = 34;
        late.update();
        late.finish();
        var lateHidden = late.full();
        late.level = 33;
        late.update();
        var lateRestored = late.full();

        var replaced = new Holder(32);
        replaced.update();
        var replacedOld = replaced.full();
        replaced.level = 34;
        replaced.update();
        replaced.level = 33;
        replaced.update();
        var replacedNew = replaced.full();
        replaced.finish();

        var extreme = new Holder(Integer.MIN_VALUE);
        extreme.update();

        System.out.println("BEGIN_RESULT");
        System.out.println("initial|" + state(initial));
        System.out.println("pending-reused|" + (first == again) + "|" + (first == beforeDemotion));
        System.out.println("demoted|" + state(first) + "|" + state(unloaded) + "|" + (initial == unloaded));
        System.out.println("before-promotion|" + (beforePromotion == unloaded));
        System.out.println("new-future|" + (second != first) + "|" + secondBeforeCompletion);
        System.out.println("completed-reused|" + (second == completed) + "|" + (hidden == unloaded) + "|" + (restored == second));
        System.out.println("duplicate-rejected|" + duplicateRejected);
        System.out.println("late-unloaded|" + state(lateOld) + "|" + (lateHidden == unloaded) + "|" + state(lateRestored));
        System.out.println("late-replacement|" + state(replacedOld) + "|" + (replacedOld != replacedNew) + "|" + state(replacedNew));
        System.out.println("overflow-level|" + state(extreme.full()));
        System.out.println("END_RESULT");
    }
}
