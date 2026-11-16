import com.mojang.serialization.Codec;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import net.minecraft.world.ticks.*;

/** Load the same saved FULL NBT as Rust, then run Java's chunk postprocessing. */
public class SavedFullPostprocessingOracle {
    static final Map<BlockPos, BlockState> blocks = new HashMap<>();
    static LevelTicks<Block> tickAccess;
    static long counter = 2;
    static long gameTime = 100;

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

    static class FakeLevel extends ServerLevel {
        FakeLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public BlockState getBlockState(BlockPos pos) {
            return blocks.getOrDefault(pos, Blocks.STONE.defaultBlockState());
        }
        @Override public RandomSource getRandom() { return RandomSource.create(42); }
        @Override public LevelTicks<Block> getBlockTicks() { return tickAccess; }
        @Override public long getGameTime() { return gameTime; }
        @Override public long nextSubTickCount() { return counter++; }
    }

    static class FakeChunk extends LevelChunk {
        FakeChunk() { super((Level) null, new ChunkPos(0, 0)); }
        @Override public ChunkPos getPos() { return new ChunkPos(0, 0); }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 16; }
        @Override public BlockState getBlockState(BlockPos pos) {
            return blocks.getOrDefault(pos, Blocks.STONE.defaultBlockState());
        }
    }

    static class FakeUpgrade extends UpgradeData {
        FakeUpgrade() { super(new CompoundTag(), LevelHeightAccessor.create(0, 16)); }
        @Override public void upgrade(LevelChunk chunk) {}
    }

    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var biomes = lookup.lookupOrThrow(Registries.BIOME);
        Holder<Biome> plains = biomes.getOrThrow(Biomes.PLAINS);
        var ids = new IdMapper<Holder<Biome>>();
        biomes.listElements().forEach(ids::add);
        var blockStrategy = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
        var biomeStrategy = Strategy.createForBiomes(ids);
        Codec<Holder<Biome>> biomeCodec = Codec.STRING.xmap(
                name -> biomes.getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse(name))),
                biome -> biome.unwrapKey().orElseThrow().identifier().toString());
        var factory = new PalettedContainerFactory(
                blockStrategy, Blocks.AIR.defaultBlockState(),
                PalettedContainer.codecRW(BlockState.CODEC, blockStrategy, Blocks.AIR.defaultBlockState()),
                biomeStrategy, plains, PalettedContainer.codecRO(biomeCodec, biomeStrategy, plains));
        var source = NbtIo.read(Path.of(args[0]));
        var parsed = SerializableChunkData.parse(LevelHeightAccessor.create(0, 16), factory, source);
        if (parsed == null || parsed.chunkStatus() != ChunkStatus.FULL)
            throw new AssertionError("saved FULL chunk was not parsed");
        var section = parsed.sectionData().stream().filter(value -> value.y() == 0)
                .findFirst().orElseThrow().chunkSection();
        if (section == null) throw new AssertionError("saved block section is missing");
        for (int y = 0; y < 16; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++)
            blocks.put(new BlockPos(x, y, z), section.getBlockState(x, y, z));
        if (!parsed.packedTicks().fluids().isEmpty() || !parsed.blockEntities().isEmpty())
            throw new AssertionError("unexpected saved fixture data");

        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var level = (FakeLevel) unsafe.allocateInstance(FakeLevel.class);
        var chunk = (FakeChunk) unsafe.allocateInstance(FakeChunk.class);
        set(chunk, "postProcessing", parsed.postProcessingSections());
        set(chunk, "pendingBlockEntities", new HashMap<BlockPos, CompoundTag>());
        set(chunk, "upgradeData", unsafe.allocateInstance(FakeUpgrade.class));
        var ticks = new LevelChunkTicks<Block>(parsed.packedTicks().blocks());
        tickAccess = new LevelTicks<Block>(pos -> true);
        tickAccess.addContainer(new ChunkPos(0, 0), ticks);
        boolean liveTick = args.length > 2 && args[2].equals("live-tick");
        boolean unpackWithLiveTick = args.length > 2 && args[2].equals("unpack-with-live-tick");
        boolean unpackOnly = args.length > 2 && (args[2].equals("unpack") || unpackWithLiveTick);
        if (unpackOnly) {
            gameTime = args.length > 3 ? Long.parseLong(args[3]) : 100L;
            counter = args.length > 4 ? Long.parseLong(args[4]) : 0L;
        }
        if (unpackWithLiveTick) {
            tickAccess.schedule(new ScheduledTick<>(Blocks.RED_SAND, new BlockPos(9, 8, 8),
                    gameTime + 2L, TickPriority.NORMAL, counter++));
        }
        if (liveTick) {
            counter = 1;
            tickAccess.schedule(new ScheduledTick<>(Blocks.RED_SAND, new BlockPos(8, 8, 8),
                    102L, TickPriority.NORMAL, 0L));
        }
        chunk.postProcessGeneration(level);
        if (liveTick || unpackOnly) ticks.unpack(gameTime);
        if (args.length > 1) {
            var pack = SerializableChunkData.class.getDeclaredMethod("packOffsets",
                    it.unimi.dsi.fastutil.shorts.ShortList[].class);
            pack.setAccessible(true);
            source.put("PostProcessing", (ListTag) pack.invoke(null, (Object) chunk.getPostProcessing()));
            NbtIo.write(source, Path.of(args[1]));
        }
        out.println("BEGIN_RESULT");
        for (var tick : ticks.pack(gameTime)) {
            var pos = tick.pos();
            out.printf("%s %d,%d,%d %d %d%n", BuiltInRegistries.BLOCK.getKey(tick.type()),
                    pos.getX(), pos.getY(), pos.getZ(), tick.delay(), tick.priority().getValue());
        }
        out.println("counter " + counter);
        out.println("END_RESULT");
    }
}
