import com.mojang.serialization.Lifecycle;
import java.io.DataInput;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import net.minecraft.SharedConstants;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.storage.RegionFile;
import net.minecraft.world.level.chunk.storage.RegionFileVersion;
import net.minecraft.world.level.chunk.storage.RegionStorageInfo;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructureStart;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePieceSerializationContext;
import net.minecraft.world.level.levelgen.structure.structures.SwampHutPiece;

public class GeneratedHutRegionsOracle {
    static CompoundTag read(Path parent, String kind, ChunkPos pos) throws Exception {
        var directory = parent.resolve(kind);
        var path = directory.resolve("r." + Math.floorDiv(pos.x(), 32) + "." + Math.floorDiv(pos.z(), 32) + ".mca");
        try (var region = new RegionFile(new RegionStorageInfo("verification", null, kind),
                path, directory, RegionFileVersion.VERSION_DEFLATE, false);
             var input = region.getChunkDataInputStream(pos)) {
            if (input == null) throw new AssertionError("missing " + kind + " chunk " + pos);
            return NbtIo.read((DataInput) input);
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var parent = Path.of(args[0]);
        var startPos = new ChunkPos(Integer.parseInt(args[1]), Integer.parseInt(args[2]));
        var mobPos = new ChunkPos(Integer.parseInt(args[3]), Integer.parseInt(args[4]));
        var chunk = read(parent, "region", startPos);
        var entities = read(parent, "entities", mobPos);
        if (!chunk.equals(NbtIo.read(parent.resolve("terrain.nbt"))))
            throw new AssertionError("terrain region differs from draft");
        if (!entities.equals(NbtIo.read(parent.resolve("entities.nbt"))))
            throw new AssertionError("entity region differs from draft");

        var lookup = VanillaRegistries.createWorldLookup();
        Structure hut = lookup.lookupOrThrow(Registries.STRUCTURE)
                .getOrThrow(ResourceKey.create(Registries.STRUCTURE, Identifier.parse("minecraft:swamp_hut"))).value();
        var registry = new MappedRegistry<Structure>(Registries.STRUCTURE, Lifecycle.stable());
        Registry.register(registry, Identifier.parse("example:hut"), hut);
        registry.freeze();
        var access = new RegistryAccess.ImmutableRegistryAccess(List.of(registry));
        var context = new StructurePieceSerializationContext(null, access, null);
        var startsMethod = SerializableChunkData.class.getDeclaredMethod("unpackStructureStart",
                StructurePieceSerializationContext.class, CompoundTag.class, long.class);
        var refsMethod = SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences",
                RegistryAccess.class, ChunkPos.class, CompoundTag.class);
        startsMethod.setAccessible(true);
        refsMethod.setAccessible(true);
        var structures = chunk.getCompoundOrEmpty("structures");
        var starts = (Map<?, ?>) startsMethod.invoke(null, context, structures, 42L);
        var refs = (Map<?, ?>) refsMethod.invoke(null, access, startPos, structures);
        var start = (StructureStart) starts.get(hut);
        if (start == null || !start.getChunkPos().equals(startPos) || start.getPieces().size() != 1
                || !(start.getPieces().get(0) instanceof SwampHutPiece))
            throw new AssertionError("invalid hut start");
        var hutRefs = (it.unimi.dsi.fastutil.longs.LongSet) refs.get(hut);
        if (hutRefs == null || !hutRefs.contains(ChunkPos.pack(startPos.x(), startPos.z())))
            throw new AssertionError("hut reference missing");
    }
}
