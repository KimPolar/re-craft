import com.mojang.serialization.Lifecycle;
import it.unimi.dsi.fastutil.longs.LongSet;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pieces.*;

/** Verify the supported part of a disposable saved-start draft in Java. */
public class ResumedTreasureDraftOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup().lookupOrThrow(Registries.STRUCTURE);
        var id = Identifier.parse("minecraft:buried_treasure");
        var structure = lookup.getOrThrow(ResourceKey.create(Registries.STRUCTURE, id)).value();
        var registry = new MappedRegistry<Structure>(Registries.STRUCTURE, Lifecycle.stable());
        Registry.register(registry, id, structure);
        registry.freeze();
        var access = new RegistryAccess.ImmutableRegistryAccess(List.of(registry));
        var context = new StructurePieceSerializationContext(null, access, null);
        var startsMethod = SerializableChunkData.class.getDeclaredMethod("unpackStructureStart", StructurePieceSerializationContext.class, CompoundTag.class, long.class);
        var refsMethod = SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences", RegistryAccess.class, ChunkPos.class, CompoundTag.class);
        var packMethod = SerializableChunkData.class.getDeclaredMethod("packStructureData", StructurePieceSerializationContext.class, ChunkPos.class, Map.class, Map.class);
        startsMethod.setAccessible(true); refsMethod.setAccessible(true); packMethod.setAccessible(true);
        var chunk = NbtIo.read(Path.of(args[0]));
        var data = chunk.getCompoundOrEmpty("structures");
        var position = new ChunkPos(3, 3);
        var starts = (Map<?, ?>) startsMethod.invoke(null, context, data, 42L);
        var refs = (Map<?, ?>) refsMethod.invoke(null, access, position, data);
        var start = (StructureStart) starts.get(structure);
        var set = (LongSet) refs.get(structure);
        if (start == null || !start.getChunkPos().equals(position) || start.getReferences() != 2 || start.getPieces().size() != 1)
            throw new AssertionError("saved treasure start differs");
        if (set == null || set.size() != 1 || !set.contains(ChunkPos.pack(3, 3))) throw new AssertionError("reference changed");
        var box = start.getPieces().get(0).getBoundingBox();
        int y = Integer.parseInt(args[1]);
        if (box.minX() != 57 || box.maxX() != 57 || box.minY() != y || box.maxY() != y || box.minZ() != 57 || box.maxZ() != 57)
            throw new AssertionError("saved treasure bounds differ");
        var repacked = (CompoundTag) packMethod.invoke(null, context, position, starts, refs);
        var reread = (Map<?, ?>) startsMethod.invoke(null, context, repacked, 42L);
        var again = (StructureStart) reread.get(structure);
        if (again == null || again.getReferences() != 2 || again.getPieces().size() != 1
                || !again.getPieces().get(0).getBoundingBox().equals(box)) throw new AssertionError("Java round trip changed treasure");
    }
}
