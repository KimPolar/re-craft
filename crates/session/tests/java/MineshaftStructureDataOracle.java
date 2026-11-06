import com.mojang.serialization.Lifecycle;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pieces.*;

public class MineshaftStructureDataOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        Structure mine = lookup.lookupOrThrow(Registries.STRUCTURE)
            .getOrThrow(ResourceKey.create(Registries.STRUCTURE, Identifier.parse("minecraft:mineshaft"))).value();
        var registry = new MappedRegistry<Structure>(Registries.STRUCTURE, Lifecycle.stable());
        Registry.register(registry, Identifier.parse("example:mine"), mine);
        registry.freeze();
        var access = new RegistryAccess.ImmutableRegistryAccess(List.of(registry));
        var context = new StructurePieceSerializationContext(null, access, null);
        var unpackStarts = SerializableChunkData.class.getDeclaredMethod("unpackStructureStart", StructurePieceSerializationContext.class, net.minecraft.nbt.CompoundTag.class, long.class);
        var unpackReferences = SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences", RegistryAccess.class, ChunkPos.class, net.minecraft.nbt.CompoundTag.class);
        var pack = SerializableChunkData.class.getDeclaredMethod("packStructureData", StructurePieceSerializationContext.class, ChunkPos.class, Map.class, Map.class);
        unpackStarts.setAccessible(true);
        unpackReferences.setAccessible(true);
        pack.setAccessible(true);
        var pos = new ChunkPos(0, 0);
        var input = NbtIo.read(Path.of(args[0]));
        var starts = (Map<?, ?>) unpackStarts.invoke(null, context, input, 42L);
        var references = (Map<?, ?>) unpackReferences.invoke(null, access, pos, input);
        var start = (StructureStart) starts.get(mine);
        if (starts.size() != 1 || start == null || !start.getChunkPos().equals(pos) || start.getPieces().size() < 2)
            throw new AssertionError("mineshaft start did not load");
        var set = (it.unimi.dsi.fastutil.longs.LongSet) references.get(mine);
        if (references.size() != 1 || set == null || set.size() != 1 || !set.contains(ChunkPos.pack(0, 0)))
            throw new AssertionError("mineshaft reference did not load");
        var output = (net.minecraft.nbt.CompoundTag) pack.invoke(null, context, pos, starts, references);
        if (!input.equals(output)) throw new AssertionError("mineshaft changed in Java: input=" + input + " output=" + output);
    }
}
