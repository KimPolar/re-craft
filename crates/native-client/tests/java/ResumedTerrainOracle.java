import com.mojang.serialization.Codec;
import java.io.DataInput;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Objects;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import net.minecraft.world.level.chunk.status.ChunkStatus;

/** Checks a disposable terrain draft with the installed Java serializer. */
public class ResumedTerrainOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup().lookupOrThrow(Registries.BIOME);
        Holder<Biome> defaultBiome = lookup.getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse("minecraft:plains")));
        var ids = new IdMapper<Holder<Biome>>();
        lookup.listElements().forEach(ids::add);
        var blockStrategy = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
        var biomeStrategy = Strategy.createForBiomes(ids);
        Codec<Holder<Biome>> biomeCodec = Codec.STRING.xmap(
            name -> lookup.getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse(name))),
            biome -> biome.unwrapKey().orElseThrow().identifier().toString());
        var blocksCodec = PalettedContainer.codecRW(BlockState.CODEC, blockStrategy, Blocks.AIR.defaultBlockState());
        var biomesCodec = PalettedContainer.codecRO(biomeCodec, biomeStrategy, defaultBiome);
        var factory = new PalettedContainerFactory(blockStrategy, Blocks.AIR.defaultBlockState(),
            blocksCodec, biomeStrategy, defaultBiome, biomesCodec);
        CompoundTag original;
        try (var input = new java.io.DataInputStream(Files.newInputStream(Path.of(args[0])))) {
            original = NbtIo.read((DataInput) input);
        }
        // This status change is confined to the disposable test input.
        boolean features = args.length > 4 && args[4].equals("features");
        var expectedStatus = features ? ChunkStatus.FEATURES : ChunkStatus.TERRAIN;
        original.putString("Status", features ? "minecraft:features" : "minecraft:terrain");
        var height = LevelHeightAccessor.create(Integer.parseInt(args[1]), Integer.parseInt(args[2]));
        var first = SerializableChunkData.parse(height, factory, original);
        if (first == null || first.chunkStatus() != expectedStatus) throw new AssertionError("generation status");
        var rewritten = first.write();
        try (var output = new java.io.DataOutputStream(Files.newOutputStream(Path.of(args[3])))) {
            NbtIo.write(rewritten, (java.io.DataOutput) output);
        }
        var second = SerializableChunkData.parse(height, factory, rewritten);
        if (second == null || second.chunkStatus() != expectedStatus || first.sectionData().size() != second.sectionData().size())
            throw new AssertionError("generation status or sections changed");
        if ((!features && !first.packedTicks().blocks().isEmpty()) || !first.packedTicks().fluids().isEmpty()
                || !first.packedTicks().equals(second.packedTicks())) throw new AssertionError("ticks changed");
        if (features && first.packedTicks().blocks().isEmpty()) throw new AssertionError("generated block tick missing");
        if (!Objects.equals(first.entities(), second.entities()) || !Objects.equals(first.blockEntities(), second.blockEntities()))
            throw new AssertionError("entities changed");
        for (var before : first.sectionData()) {
            var after = second.sectionData().stream().filter(section -> section.y() == before.y()).findFirst().orElseThrow();
            if (before.chunkSection() == null || after.chunkSection() == null) throw new AssertionError("missing section");
            var sourceSections = original.getListOrEmpty("sections");
            CompoundTag raw = null;
            for (int i = 0; i < sourceSections.size(); i++) {
                var candidate = sourceSections.getCompoundOrEmpty(i);
                if (candidate.getByteOr("Y", (byte) 127) == before.y()) { raw = candidate; break; }
            }
            if (raw == null) throw new AssertionError("source section missing");
            var expectedBlocks = blocksCodec.parse(NbtOps.INSTANCE, raw.get("block_states")).getOrThrow();
            var expectedBiomes = biomesCodec.parse(NbtOps.INSTANCE, raw.get("biomes")).getOrThrow();
            for (int y = 0; y < 16; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++)
                if (expectedBlocks.get(x, y, z) != before.chunkSection().getBlockState(x, y, z) || before.chunkSection().getBlockState(x, y, z) != after.chunkSection().getBlockState(x, y, z)) throw new AssertionError("block changed");
            for (int y = 0; y < 4; y++) for (int z = 0; z < 4; z++) for (int x = 0; x < 4; x++)
                if (!expectedBiomes.get(x, y, z).equals(before.chunkSection().getNoiseBiome(x, y, z)) || !before.chunkSection().getNoiseBiome(x, y, z).equals(after.chunkSection().getNoiseBiome(x, y, z))) throw new AssertionError("biome changed");
        }
        for (var key : new String[]{"structures", "PostProcessing"})
            if (!Objects.equals(original.get(key), rewritten.get(key))) throw new AssertionError(key + " changed");
        if (features) {
            for (var key : new String[]{"block_entities", "block_ticks", "fluid_ticks", "entities"})
                if (!Objects.equals(original.get(key), rewritten.get(key))) throw new AssertionError(key + " changed");
            var protoEntities = original.getListOrEmpty("entities");
            if (protoEntities.size() != 4 || protoEntities.getCompoundOrEmpty(0).getIntOr("n", -1) != 11
                    || protoEntities.getCompoundOrEmpty(1).getIntOr("n", -1) != 21
                    || !protoEntities.getCompoundOrEmpty(2).isEmpty()
                    || protoEntities.getCompoundOrEmpty(3).getIntOr("n", -1) != 22)
                throw new AssertionError("proto entity order changed");
            boolean chest = false;
            var entities = original.getListOrEmpty("block_entities");
            for (int i = 0; i < entities.size(); i++)
                if (entities.getCompoundOrEmpty(i).getStringOr("id", "").equals("minecraft:chest")) chest = true;
            if (!chest) throw new AssertionError("generated chest entity missing");
        }
        var beforeMaps = original.getCompoundOrEmpty("Heightmaps");
        var afterMaps = rewritten.getCompoundOrEmpty("Heightmaps");
        for (var key : new String[]{"WORLD_SURFACE_WG", "OCEAN_FLOOR_WG"})
            if (beforeMaps.get(key) == null || !Objects.equals(beforeMaps.get(key), afterMaps.get(key))) throw new AssertionError(key + " changed");
        for (var key : new String[]{"WORLD_SURFACE", "OCEAN_FLOOR", "MOTION_BLOCKING", "MOTION_BLOCKING_NO_LEAVES"})
            if (beforeMaps.get(key) != null && !Objects.equals(beforeMaps.get(key), afterMaps.get(key))) throw new AssertionError(key + " changed");
    }
}
