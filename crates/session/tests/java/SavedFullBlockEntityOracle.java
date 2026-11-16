import com.mojang.serialization.Codec;
import java.io.PrintStream;
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
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import net.minecraft.world.ticks.LevelChunkTicks;

/** Deserialize saved FULL block entities and run Java's actual postprocessing. */
public class SavedFullBlockEntityOracle {
    static PalettedContainerFactory factory;
    static class FakeLevel extends ServerLevel {
        FakeLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 16; }
        @Override public long getGameTime() { return 100L; }
        @Override public PalettedContainerFactory palettedContainerFactory() { return factory; }
        @Override public RegistryAccess registryAccess() {
            return RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        }
    }

    static void save(String label, LevelChunk chunk, HolderLookup.Provider lookup,
                     Path directory, PrintStream out) throws Exception {
        out.print(label);
        for (var pos : chunk.getBlockEntitiesPos()) {
            out.printf(" %d,%d,%d", pos.getX(), pos.getY(), pos.getZ());
            NbtIo.write(chunk.getBlockEntityNbtForSaving(pos, lookup),
                    directory.resolve(label + "-" + pos.getX() + "-" + pos.getY() + "-" + pos.getZ() + ".nbt"));
        }
        out.println();
    }

    public static void main(String[] args) throws Exception {
        var errors = System.err;
        try {
            run(args);
        } catch (Throwable error) {
            error.printStackTrace(errors);
            errors.flush();
            throw error;
        }
    }

    static void run(String[] args) throws Exception {
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
        factory = new PalettedContainerFactory(
                blockStrategy, Blocks.AIR.defaultBlockState(),
                PalettedContainer.codecRW(BlockState.CODEC, blockStrategy, Blocks.AIR.defaultBlockState()),
                biomeStrategy, plains, PalettedContainer.codecRO(biomeCodec, biomeStrategy, plains));
        var parsed = SerializableChunkData.parse(LevelHeightAccessor.create(0, 16), factory,
                NbtIo.read(Path.of(args[0])));
        int expectedCount = args.length > 2 ? Integer.parseInt(args[2]) : 27;
        if (parsed == null || parsed.chunkStatus() != ChunkStatus.FULL || parsed.blockEntities().size() != expectedCount)
            throw new AssertionError("invalid saved FULL fixture");
        var section = parsed.sectionData().stream().filter(value -> value.y() == 0)
                .findFirst().orElseThrow().chunkSection();
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var level = (FakeLevel) unsafe.allocateInstance(FakeLevel.class);
        var chunk = new LevelChunk(level, new ChunkPos(0, 0), UpgradeData.EMPTY,
                new LevelChunkTicks<>(parsed.packedTicks().blocks()),
                new LevelChunkTicks<>(parsed.packedTicks().fluids()), 0L,
                new LevelChunkSection[] { section }, null, null);
        var deferred = new LevelChunk(level, new ChunkPos(0, 0), UpgradeData.EMPTY,
                new LevelChunkTicks<>(parsed.packedTicks().blocks()),
                new LevelChunkTicks<>(parsed.packedTicks().fluids()), 0L,
                new LevelChunkSection[] { section }, null, null);
        var load = SerializableChunkData.class.getDeclaredMethod("lambda$postLoadChunk$0",
                List.class, ServerLevel.class, List.class, LevelChunk.class);
        load.setAccessible(true);
        load.invoke(null, List.of(), level, parsed.blockEntities(), chunk);
        load.invoke(null, List.of(), level, parsed.blockEntities(), deferred);
        chunk.postProcessGeneration(level);
        out.println("BEGIN_RESULT");
        save("S", deferred, lookup, Path.of(args[1]), out);
        deferred.postProcessGeneration(level);
        save("T", deferred, lookup, Path.of(args[1]), out);
        save("O", chunk, lookup, Path.of(args[1]), out);
        out.println("END_RESULT");
    }
}
