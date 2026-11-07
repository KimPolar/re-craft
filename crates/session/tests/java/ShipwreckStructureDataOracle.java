import com.mojang.serialization.Lifecycle;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
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
import net.minecraft.world.level.block.Rotation;
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
    }
}
