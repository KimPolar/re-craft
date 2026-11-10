import com.mojang.serialization.Lifecycle;
import it.unimi.dsi.fastutil.longs.LongSet;
import java.nio.file.Path;
import java.util.Arrays;
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
import net.minecraft.world.level.levelgen.structure.Structure;

public class SavedStructureReferencesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registry = new MappedRegistry<Structure>(Registries.STRUCTURE, Lifecycle.stable());
        for (String name : List.of("minecraft:buried_treasure", "minecraft:swamp_hut")) {
            var id = Identifier.parse(name);
            var structure = lookup.lookupOrThrow(Registries.STRUCTURE).getOrThrow(ResourceKey.create(Registries.STRUCTURE, id)).value();
            Registry.register(registry, id, structure);
        }
        registry.freeze();
        var access = new RegistryAccess.ImmutableRegistryAccess(List.of(registry));
        var read = SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences", RegistryAccess.class, ChunkPos.class, CompoundTag.class);
        read.setAccessible(true);
        var root = NbtIo.read(Path.of(args[0]));
        for (var item : (ListTag)root.get("cases")) {
            var test = (CompoundTag)item;
            var pos = new ChunkPos(test.getIntOr("x",0),test.getIntOr("z",0));
            var actual = (Map<Structure, LongSet>)read.invoke(null, access, pos, test.getCompoundOrEmpty("structures"));
            var expected = test.getCompoundOrEmpty("expected");
            if (actual.size() != expected.size()) throw new AssertionError("reference map size differs");
            for (var entry : actual.entrySet()) {
                var id = registry.getKey(entry.getKey()).toString();
                var values = expected.getLongArray(id).orElseThrow();
                if (!Arrays.equals(entry.getValue().toLongArray(), values)) throw new AssertionError("reference order differs: " + id);
            }
        }
    }
}
