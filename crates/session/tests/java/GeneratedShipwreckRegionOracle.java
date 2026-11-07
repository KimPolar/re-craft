import com.mojang.serialization.Codec;
import com.mojang.serialization.Lifecycle;
import java.io.DataInput;
import java.io.DataOutput;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.storage.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pieces.*;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class GeneratedShipwreckRegionOracle {
    private static boolean sameLight(DataLayer a, DataLayer b) {
        return a == null ? b == null : b != null && Arrays.equals(a.getData(), b.getData());
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var directory = Path.of(args[0]);
        var templateId = Identifier.parse(args[1]);
        var template = new StructureTemplate();
        var name = args[1].split(":", 2);
        try (var stream = GeneratedShipwreckRegionOracle.class.getClassLoader().getResourceAsStream(
                "data/" + name[0] + "/structure/" + name[1] + ".nbt")) {
            if (stream == null) throw new AssertionError("shipwreck template missing");
            template.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(stream, NbtAccounter.unlimitedHeap()));
        }
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var manager = (StructureTemplateManager)unsafe.allocateInstance(StructureTemplateManager.class);
        var repository = StructureTemplateManager.class.getDeclaredField("structureRepository");
        repository.setAccessible(true);
        repository.set(manager, new HashMap<>(Map.of(templateId, Optional.of(template))));
        Structure shipwreck = VanillaRegistries.createWorldLookup().lookupOrThrow(Registries.STRUCTURE)
            .getOrThrow(ResourceKey.create(Registries.STRUCTURE, Identifier.parse("minecraft:shipwreck"))).value();
        var registry = new MappedRegistry<Structure>(Registries.STRUCTURE, Lifecycle.stable());
        Registry.register(registry, Identifier.parse("example:test"), shipwreck);
        registry.freeze();
        var access = new RegistryAccess.ImmutableRegistryAccess(List.of(registry));
        var context = new StructurePieceSerializationContext(null, access, manager);
        var unpack = SerializableChunkData.class.getDeclaredMethod("unpackStructureStart", StructurePieceSerializationContext.class, CompoundTag.class, long.class);
        var unpackRefs = SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences", RegistryAccess.class, ChunkPos.class, CompoundTag.class);
        var pack = SerializableChunkData.class.getDeclaredMethod("packStructureData", StructurePieceSerializationContext.class, ChunkPos.class, Map.class, Map.class);
        unpack.setAccessible(true); unpackRefs.setAccessible(true); pack.setAccessible(true);
        var biome = Holder.direct((Biome)unsafe.allocateInstance(Biome.class));
        var ids = new IdMapper<Holder<Biome>>(); ids.add(biome);
        var blocks = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
        var biomes = Strategy.createForBiomes(ids);
        Codec<Holder<Biome>> biomeCodec = Codec.STRING.xmap(value -> {
            if (!value.equals("minecraft:plains")) throw new IllegalArgumentException(value);
            return biome;
        }, value -> "minecraft:plains");
        var factory = new PalettedContainerFactory(blocks, Blocks.AIR.defaultBlockState(),
            PalettedContainer.codecRW(BlockState.CODEC, blocks, Blocks.AIR.defaultBlockState()),
            biomes, biome, PalettedContainer.codecRO(biomeCodec, biomes, biome));
        boolean sawStart = false, sawReference = false;
        for (int i = 2; i < args.length; i += 2) {
            var pos = new ChunkPos(Integer.parseInt(args[i]), Integer.parseInt(args[i + 1]));
            var path = directory.resolve("r." + Math.floorDiv(pos.x(), 32) + "." + Math.floorDiv(pos.z(), 32) + ".mca");
            try (var region = new RegionFile(new RegionStorageInfo("verification", null, "region"),
                    path, directory, RegionFileVersion.VERSION_DEFLATE, false)) {
                CompoundTag chunk;
                try (var input = region.getChunkDataInputStream(pos)) {
                    if (input == null) throw new AssertionError("generated chunk missing: " + pos);
                    chunk = NbtIo.read((DataInput)input);
                }
                var structures = chunk.getCompoundOrEmpty("structures");
                var starts = (Map<?,?>)unpack.invoke(null, context, structures, 63L);
                var refs = (Map<?,?>)unpackRefs.invoke(null, access, pos, structures);
                var start = (StructureStart)starts.get(shipwreck);
                if (start != null) {
                    if (!start.getChunkPos().equals(pos) || start.getPieces().size() != 1)
                        throw new AssertionError("incorrect shipwreck start");
                    sawStart = true;
                }
                sawReference |= refs.containsKey(shipwreck);
                if (!structures.equals(pack.invoke(null, context, pos, starts, refs)))
                    throw new AssertionError("structure start/reference changed on reload");
                var parsed = SerializableChunkData.parse(LevelHeightAccessor.create(0, 48), factory, chunk);
                if (parsed == null) throw new AssertionError("generated chunk parse failed");
                var output = parsed.write();
                var reread = SerializableChunkData.parse(LevelHeightAccessor.create(0, 48), factory, output);
                if (reread == null || parsed.sectionData().size() != reread.sectionData().size())
                    throw new AssertionError("section count changed");
                for (var original : parsed.sectionData()) {
                    var copy = reread.sectionData().stream().filter(s -> s.y() == original.y()).findFirst()
                        .orElseThrow(() -> new AssertionError("section missing"));
                    if (!sameLight(original.blockLight(), copy.blockLight()) || !sameLight(original.skyLight(), copy.skyLight()))
                        throw new AssertionError("light changed: " + original.y());
                    var before = original.chunkSection(); var after = copy.chunkSection();
                    if (before == null || after == null) {
                        if (before != after) throw new AssertionError("section blocks missing");
                        continue;
                    }
                    for (int y = 0; y < 16; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++)
                        if (before.getBlockState(x,y,z) != after.getBlockState(x,y,z)) throw new AssertionError("block changed");
                    for (int y = 0; y < 4; y++) for (int z = 0; z < 4; z++) for (int x = 0; x < 4; x++)
                        if (before.getNoiseBiome(x,y,z) != after.getNoiseBiome(x,y,z)) throw new AssertionError("biome changed");
                }
                var beforeMetadata = chunk.copy(); beforeMetadata.remove("sections");
                var afterMetadata = output.copy(); afterMetadata.remove("sections");
                if (!beforeMetadata.equals(afterMetadata)) throw new AssertionError("chunk metadata changed");
                try (var outputStream = region.getChunkDataOutputStream(pos)) { NbtIo.write(output, (DataOutput)outputStream); }
            }
        }
        if (!sawStart || !sawReference) throw new AssertionError("missing shipwreck start or reference");
    }
}
