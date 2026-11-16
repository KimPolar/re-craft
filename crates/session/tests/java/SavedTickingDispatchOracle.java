import com.mojang.serialization.Codec;
import java.nio.file.Path;
import java.util.*;
import java.util.concurrent.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.*;
import net.minecraft.util.RandomSource;
import net.minecraft.util.thread.BlockableEventLoop;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.ticks.*;

/** Feed real saved NBT through ChunkMap's ticking callback and tick unpack. */
public class SavedTickingDispatchOracle {
    static final java.util.Map<BlockPos, BlockState> states = new HashMap<>();
    static LevelTicks<Block> levelTicks;
    static long gameTime = 100;
    static long nextSubTick = 2;

    static void set(Object object, String name, Object value) throws Exception {
        for (Class<?> type = object.getClass(); type != null; type = type.getSuperclass()) {
            try {
                var field = type.getDeclaredField(name);
                field.setAccessible(true);
                field.set(object, value);
                return;
            } catch (NoSuchFieldException ignored) {}
        }
        throw new NoSuchFieldException(name);
    }

    static class Executor extends BlockableEventLoop<Runnable> {
        final ArrayDeque<Runnable> queue = new ArrayDeque<>();
        Executor() { super("saved-ticking-oracle", false); }
        @Override protected boolean shouldRun(Runnable task) { return true; }
        @Override protected Thread getRunningThread() { return null; }
        @Override public Runnable wrapRunnable(Runnable task) { return task; }
        @Override public void execute(Runnable task) { queue.add(task); }
        void drain() { while (!queue.isEmpty()) queue.remove().run(); }
    }

    static class Level extends ServerLevel {
        Level() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public BlockState getBlockState(BlockPos pos) {
            return states.getOrDefault(pos, Blocks.STONE.defaultBlockState());
        }
        @Override public RandomSource getRandom() { return RandomSource.create(42); }
        @Override public LevelTicks<Block> getBlockTicks() { return levelTicks; }
        @Override public long getGameTime() { return gameTime; }
        @Override public long nextSubTickCount() { return nextSubTick++; }
    }

    static class Chunk extends LevelChunk {
        ChunkPos position;
        Chunk() { super((net.minecraft.world.level.Level) null, new ChunkPos(0, 0)); }
        @Override public ChunkPos getPos() { return position; }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 16; }
        @Override public BlockState getBlockState(BlockPos pos) {
            return states.getOrDefault(pos, Blocks.STONE.defaultBlockState());
        }
    }

    static class Upgrade extends UpgradeData {
        Upgrade() { super(new net.minecraft.nbt.CompoundTag(), LevelHeightAccessor.create(0, 16)); }
        @Override public void upgrade(LevelChunk chunk) {}
    }

    static class Holder extends ChunkHolder {
        ChunkPos position;
        CompletableFuture<ChunkResult<ChunkAccess>> full;
        CompletableFuture<Object> send;
        Holder() { super(null, 0, null, null, null, null); }
        @Override public ChunkPos getPos() { return position; }
        @Override public CompletableFuture<ChunkResult<ChunkAccess>> scheduleChunkGenerationTask(
                ChunkStatus status, ChunkMap map) {
            if (status != ChunkStatus.FULL) throw new AssertionError("not FULL");
            return full;
        }
        @Override public CompletableFuture<?> getSendSyncFuture() { return send; }
    }

    static class Map extends ChunkMap {
        java.util.Map<Long, Holder> holders;
        Map() { super(null, null, null, null, null, null, null, null, null, null, 0, false); }
        @Override public ChunkHolder getUpdatingChunkIfPresent(long pos) { return holders.get(pos); }
    }

    static void runOverlapping(SerializableChunkData parsed, LevelChunkSection section,
            sun.misc.Unsafe unsafe, java.io.PrintStream out) throws Exception {
        states.clear();
        for (int chunkX = 0; chunkX <= 1; chunkX++)
            for (int y = 0; y < 16; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++)
                states.put(new BlockPos(x + chunkX * 16, y, z), section.getBlockState(x, y, z));
        var map = (Map) unsafe.allocateInstance(Map.class);
        map.holders = new HashMap<>();
        var executor = new Executor();
        set(map, "mainThreadExecutor", executor);
        set(map, "level", unsafe.allocateInstance(Level.class));
        levelTicks = new LevelTicks<>(pos -> true);
        var holders = new HashMap<ChunkPos, Holder>();
        var chunks = new HashMap<ChunkPos, Chunk>();
        var queues = new HashMap<ChunkPos, LevelChunkTicks<Block>>();
        var originalMarks = parsed.postProcessingSections();
        for (int z = -1; z <= 1; z++) for (int x = -1; x <= 2; x++) {
            var pos = new ChunkPos(x, z);
            var chunk = (Chunk) unsafe.allocateInstance(Chunk.class);
            chunk.position = pos;
            var marks = new it.unimi.dsi.fastutil.shorts.ShortList[originalMarks.length];
            if (z == 0 && (x == 0 || x == 1)) {
                for (int i = 0; i < marks.length; i++) {
                    if (originalMarks[i] != null)
                        marks[i] = new it.unimi.dsi.fastutil.shorts.ShortArrayList(originalMarks[i]);
                }
            }
            set(chunk, "postProcessing", marks);
            set(chunk, "pendingBlockEntities", new HashMap<BlockPos, net.minecraft.nbt.CompoundTag>());
            set(chunk, "upgradeData", unsafe.allocateInstance(Upgrade.class));
            var saved = x == 0 && z == 0 ? parsed.packedTicks().blocks() : List.of(new SavedTick<>(
                    Blocks.SAND, new BlockPos(x * 16 + 8, 8, z * 16 + 8), 7, TickPriority.NORMAL));
            var ticks = new LevelChunkTicks<Block>(saved);
            set(chunk, "blockTicks", ticks);
            set(chunk, "fluidTicks", new LevelChunkTicks<Fluid>());
            levelTicks.addContainer(pos, ticks);
            var holder = (Holder) unsafe.allocateInstance(Holder.class);
            holder.position = pos;
            holder.full = new CompletableFuture<>();
            holder.send = new CompletableFuture<>();
            holders.put(pos, holder);
            chunks.put(pos, chunk);
            queues.put(pos, ticks);
            map.holders.put(pos.pack(), holder);
        }
        var a = new ChunkPos(0, 0);
        var b = new ChunkPos(1, 0);
        var first = map.prepareTickingChunk(holders.get(a));
        var second = map.prepareTickingChunk(holders.get(b));
        gameTime = 500;
        nextSubTick = 2;
        for (int z = -1; z <= 1; z++) for (int x = -1; x <= 1; x++) {
            var pos = new ChunkPos(x, z);
            holders.get(pos).full.complete(ChunkResult.of(chunks.get(pos)));
        }
        executor.drain();
        for (int z = -1; z <= 1; z++) {
            var pos = new ChunkPos(2, z);
            holders.get(pos).full.complete(ChunkResult.of(chunks.get(pos)));
        }
        executor.drain();
        var again = map.prepareTickingChunk(holders.get(a));
        executor.drain();
        out.println("BEGIN_RESULT");
        out.println("first " + first.join().isSuccess());
        out.println("second " + second.join().isSuccess());
        out.println("again " + again.join().isSuccess());
        for (var entry : List.of(java.util.Map.entry("a", a), java.util.Map.entry("b", b))) {
            for (var tick : queues.get(entry.getValue()).pack(500)) {
                var pos = tick.pos();
                out.printf("%s %s %d,%d,%d %d %d%n", entry.getKey(),
                        BuiltInRegistries.BLOCK.getKey(tick.type()), pos.getX(), pos.getY(), pos.getZ(),
                        tick.delay(), tick.priority().getValue());
            }
        }
        var neighbor = queues.get(new ChunkPos(-1, -1));
        out.printf("neighbor %d %d%n", neighbor.pack(500).get(0).delay(), neighbor.pack(1000).get(0).delay());
        out.println("counter " + nextSubTick);
        levelTicks.tick(502, 65536, (position, block) -> out.printf("run %s %d,%d,%d%n",
                BuiltInRegistries.BLOCK.getKey(block), position.getX(), position.getY(), position.getZ()));
        out.println("END_RESULT");
    }

    static void run(String[] args, java.io.PrintStream out) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var biomes = lookup.lookupOrThrow(Registries.BIOME);
        net.minecraft.core.Holder<Biome> plains = biomes.getOrThrow(Biomes.PLAINS);
        var ids = new IdMapper<net.minecraft.core.Holder<Biome>>();
        biomes.listElements().forEach(ids::add);
        var blocks = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
        var biome = Strategy.createForBiomes(ids);
        Codec<net.minecraft.core.Holder<Biome>> codec = Codec.STRING.xmap(
                name -> biomes.getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse(name))),
                value -> value.unwrapKey().orElseThrow().identifier().toString());
        var factory = new PalettedContainerFactory(blocks, Blocks.AIR.defaultBlockState(),
                PalettedContainer.codecRW(BlockState.CODEC, blocks, Blocks.AIR.defaultBlockState()),
                biome, plains, PalettedContainer.codecRO(codec, biome, plains));
        var parsed = SerializableChunkData.parse(LevelHeightAccessor.create(0, 16), factory, NbtIo.read(Path.of(args[0])));
        var section = parsed.sectionData().stream().filter(value -> value.y() == 0).findFirst().orElseThrow().chunkSection();
        for (int y = 0; y < 16; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++)
            states.put(new BlockPos(x, y, z), section.getBlockState(x, y, z));
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        if (args.length > 1 && args[1].equals("overlap")) {
            runOverlapping(parsed, section, unsafe, out);
            return;
        }
        var map = (Map) unsafe.allocateInstance(Map.class);
        map.holders = new HashMap<>();
        var executor = new Executor();
        set(map, "mainThreadExecutor", executor);
        set(map, "level", unsafe.allocateInstance(Level.class));
        levelTicks = new LevelTicks<>(pos -> true);
        var holders = new Holder[9];
        var chunks = new Chunk[9];
        @SuppressWarnings("unchecked")
        var ticks = (LevelChunkTicks<Block>[]) new LevelChunkTicks<?>[9];
        for (int i = 0; i < 9; i++) {
            var pos = new ChunkPos(i % 3 - 1, i / 3 - 1);
            var chunk = (Chunk) unsafe.allocateInstance(Chunk.class);
            chunk.position = pos;
            set(chunk, "postProcessing", parsed.postProcessingSections());
            set(chunk, "pendingBlockEntities", new HashMap<BlockPos, net.minecraft.nbt.CompoundTag>());
            set(chunk, "upgradeData", unsafe.allocateInstance(Upgrade.class));
            var saved = i == 4 ? parsed.packedTicks().blocks() : List.of(new SavedTick<>(Blocks.SAND,
                    new BlockPos(pos.x() * 16 + 8, 8, pos.z() * 16 + 8), 7, TickPriority.NORMAL));
            ticks[i] = new LevelChunkTicks<>(saved);
            set(chunk, "blockTicks", ticks[i]);
            set(chunk, "fluidTicks", new LevelChunkTicks<Fluid>());
            levelTicks.addContainer(pos, ticks[i]);
            chunks[i] = chunk;
            var holder = (Holder) unsafe.allocateInstance(Holder.class);
            holder.position = pos;
            holder.full = new CompletableFuture<>();
            holder.send = new CompletableFuture<>();
            holders[i] = holder;
            map.holders.put(pos.pack(), holder);
        }
        var future = map.prepareTickingChunk(holders[4]);
        for (int i = 0; i < 8; i++) holders[i].full.complete(ChunkResult.of(chunks[i]));
        out.println("BEGIN_RESULT");
        out.printf("before %s %d %d%n", future.isDone(), ticks[4].pack(100).size(), ticks[0].pack(100).get(0).delay());
        gameTime = 500;
        holders[8].full.complete(ChunkResult.of(chunks[8]));
        out.printf("queued %s %d %d%n", future.isDone(), ticks[4].pack(500).size(), executor.queue.size());
        executor.drain();
        out.println("success " + future.join().isSuccess());
        for (var tick : ticks[4].pack(500)) {
            var pos = tick.pos();
            out.printf("%s %d,%d,%d %d %d%n", BuiltInRegistries.BLOCK.getKey(tick.type()),
                    pos.getX(), pos.getY(), pos.getZ(), tick.delay(), tick.priority().getValue());
        }
        out.printf("neighbor %d %d%n", ticks[0].pack(500).get(0).delay(), ticks[0].pack(1000).get(0).delay());
        out.println("counter " + nextSubTick);
        out.println("END_RESULT");
    }

    public static void main(String[] args) throws Exception {
        var out = System.out;
        var errors = System.err;
        try { run(args, out); }
        catch (Throwable error) { error.printStackTrace(errors); throw error; }
    }
}
