import com.mojang.serialization.Lifecycle;
import com.mojang.serialization.Codec;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
import java.io.DataInput;
import java.io.DataOutput;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.storage.RegionFile;
import net.minecraft.world.level.chunk.storage.RegionFileVersion;
import net.minecraft.world.level.chunk.storage.RegionStorageInfo;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pieces.*;
import net.minecraft.world.level.levelgen.structure.structures.ShipwreckPieces.ShipwreckPiece;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class ShipwreckStructureDataOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        Structure shipwreck = lookup.lookupOrThrow(Registries.STRUCTURE)
            .getOrThrow(ResourceKey.create(Registries.STRUCTURE, Identifier.parse("minecraft:shipwreck"))).value();
        var registry = new MappedRegistry<Structure>(Registries.STRUCTURE, Lifecycle.stable());
        Registry.register(registry, Identifier.parse("example:wreck"), shipwreck); registry.freeze();
        var access = new RegistryAccess.ImmutableRegistryAccess(List.of(registry));
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var manager = (StructureTemplateManager)((sun.misc.Unsafe)unsafeField.get(null)).allocateInstance(StructureTemplateManager.class);
        var repository = StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var template = new StructureTemplate(); var size = StructureTemplate.class.getDeclaredField("size");
        size.setAccessible(true); size.set(template, new Vec3i(7,4,5));
        var id = Identifier.withDefaultNamespace("shipwreck/with_mast");
        repository.set(manager, new HashMap<>(Map.of(id, Optional.of(template))));
        var context = new StructurePieceSerializationContext(null, access, manager);
        var pos = new ChunkPos(0, 0);
        var piece = new ShipwreckPiece(manager, id, new BlockPos(0,90,0), Rotation.NONE, false);
        var start = new StructureStart(shipwreck, pos, 0, new PiecesContainer(List.of(piece)));
        var set = new LongOpenHashSet(); set.add(ChunkPos.pack(0,0));
        var pack = SerializableChunkData.class.getDeclaredMethod("packStructureData", StructurePieceSerializationContext.class,ChunkPos.class,Map.class,Map.class);
        var unpack = SerializableChunkData.class.getDeclaredMethod("unpackStructureStart", StructurePieceSerializationContext.class,CompoundTag.class,long.class);
        var unpackRefs = SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences", RegistryAccess.class,ChunkPos.class,CompoundTag.class);
        pack.setAccessible(true); unpack.setAccessible(true); unpackRefs.setAccessible(true);
        var expected = (CompoundTag)pack.invoke(null,context,pos,Map.of(shipwreck,start),Map.of(shipwreck,set));
        NbtIo.write(expected,Path.of(args[0]));
        var rust = NbtIo.read(Path.of(args[1]));
        if (!expected.equals(rust)) throw new AssertionError("Rust differs: " + rust + " expected=" + expected);
        var starts = (Map<?,?>)unpack.invoke(null,context,rust,42L);
        var refs = (Map<?,?>)unpackRefs.invoke(null,access,pos,rust);
        var restored = (StructureStart)starts.get(shipwreck);
        if (restored == null || restored.getPieces().size() != 1 || refs.size() != 1) throw new AssertionError("Shipwreck start or reference missing");
        var roundTrip = (CompoundTag)pack.invoke(null,context,pos,starts,refs);
        if (!rust.equals(roundTrip)) throw new AssertionError("Java reload changed shipwreck structure");
        if (args.length > 2) {
            var directory = Path.of(args[2]);
            try (var region = new RegionFile(new RegionStorageInfo("verification", null, "region"),
                    directory.resolve("r.0.0.mca"), directory, RegionFileVersion.VERSION_DEFLATE, false)) {
                CompoundTag chunk;
                try (var input = region.getChunkDataInputStream(pos)) {
                    if (input == null) throw new AssertionError("region chunk missing");
                    chunk = NbtIo.read((DataInput)input);
                }
                if (!chunk.getCompoundOrEmpty("structures").equals(rust)) throw new AssertionError("region structures differ");
                var biome = Holder.direct((Biome)((sun.misc.Unsafe)unsafeField.get(null)).allocateInstance(Biome.class));
                var ids = new IdMapper<Holder<Biome>>(); ids.add(biome);
                var blocks = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
                var biomes = Strategy.createForBiomes(ids);
                Codec<Holder<Biome>> biomeCodec = Codec.STRING.xmap(name -> {
                    if (!name.equals("minecraft:plains")) throw new IllegalArgumentException(name);
                    return biome;
                }, value -> "minecraft:plains");
                var factory = new PalettedContainerFactory(blocks, Blocks.AIR.defaultBlockState(),
                    PalettedContainer.codecRW(BlockState.CODEC, blocks, Blocks.AIR.defaultBlockState()),
                    biomes, biome, PalettedContainer.codecRO(biomeCodec, biomes, biome));
                var parsed = SerializableChunkData.parse(LevelHeightAccessor.create(0,16), factory, chunk);
                if (parsed == null || parsed.sectionData().size() != 1 ||
                    parsed.sectionData().get(0).chunkSection().getBlockState(0,8,0).getBlock() != Blocks.OAK_PLANKS)
                    throw new AssertionError("region block state differs");
                if (parsed.sectionData().get(0).chunkSection().getBlockState(1,8,0).getBlock() != Blocks.CHEST)
                    throw new AssertionError("region chest missing");
                var output = parsed.write();
                if (!output.getCompoundOrEmpty("structures").equals(rust)) throw new AssertionError("chunk rewrite changed structures");
                if (!Objects.equals(chunk.get("block_entities"), output.get("block_entities"))) throw new AssertionError("chunk rewrite changed chest data");
                var reread = SerializableChunkData.parse(LevelHeightAccessor.create(0,16), factory, output);
                if (reread == null || reread.sectionData().size() != 1) throw new AssertionError("rewritten section missing");
                var before = parsed.sectionData().get(0).chunkSection();
                var after = reread.sectionData().get(0).chunkSection();
                for (int y = 0; y < 16; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++)
                    if (before.getBlockState(x,y,z) != after.getBlockState(x,y,z)) throw new AssertionError("block changed on rewrite");
                for (int y = 0; y < 4; y++) for (int z = 0; z < 4; z++) for (int x = 0; x < 4; x++)
                    if (before.getNoiseBiome(x,y,z) != biome || after.getNoiseBiome(x,y,z) != biome) throw new AssertionError("biome changed on rewrite");
                var beforeMetadata = chunk.copy(); beforeMetadata.remove("sections");
                var afterMetadata = output.copy(); afterMetadata.remove("sections");
                if (!beforeMetadata.equals(afterMetadata)) throw new AssertionError("chunk metadata changed on rewrite");
                try (var stream = region.getChunkDataOutputStream(pos)) { NbtIo.write(output, (DataOutput)stream); }
            }
        }
    }
}
