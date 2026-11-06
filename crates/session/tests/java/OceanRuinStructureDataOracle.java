import com.mojang.serialization.Lifecycle;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
import java.lang.reflect.Field;
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
import net.minecraft.world.level.levelgen.structure.structures.OceanRuinPieces.OceanRuinPiece;
import net.minecraft.world.level.levelgen.structure.structures.OceanRuinStructure;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class OceanRuinStructureDataOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        Structure ruin = lookup.lookupOrThrow(Registries.STRUCTURE)
            .getOrThrow(ResourceKey.create(Registries.STRUCTURE, Identifier.parse("minecraft:ocean_ruin_warm"))).value();
        var registry = new MappedRegistry<Structure>(Registries.STRUCTURE, Lifecycle.stable());
        Registry.register(registry, Identifier.parse("example:ruin"), ruin); registry.freeze();
        var access = new RegistryAccess.ImmutableRegistryAccess(List.of(registry));
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var manager = (StructureTemplateManager)((sun.misc.Unsafe)f.get(null)).allocateInstance(StructureTemplateManager.class);
        var repository = StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var template = new StructureTemplate(); var size = StructureTemplate.class.getDeclaredField("size");
        size.setAccessible(true); size.set(template, new Vec3i(7,4,5));
        var id = Identifier.withDefaultNamespace("underwater_ruin/warm_1");
        var templates = new HashMap<Identifier, Optional<StructureTemplate>>(); templates.put(id, Optional.of(template)); repository.set(manager, templates);
        var context = new StructurePieceSerializationContext(null, access, manager);
        var pos = new ChunkPos(0, 0);
        var pieces = List.of(
            new OceanRuinPiece(manager,id,new BlockPos(1,70,2),Rotation.NONE,0.7f,OceanRuinStructure.Type.WARM,false),
            new OceanRuinPiece(manager,id,new BlockPos(5,71,6),Rotation.CLOCKWISE_90,0.5f,OceanRuinStructure.Type.WARM,true));
        var start = new StructureStart(ruin,pos,0,new PiecesContainer(new ArrayList<StructurePiece>(pieces)));
        var set = new LongOpenHashSet(); set.add(ChunkPos.pack(0,0));
        var pack = SerializableChunkData.class.getDeclaredMethod("packStructureData", StructurePieceSerializationContext.class,ChunkPos.class,Map.class,Map.class);
        var unpack = SerializableChunkData.class.getDeclaredMethod("unpackStructureStart", StructurePieceSerializationContext.class,CompoundTag.class,long.class);
        var unpackRefs = SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences", RegistryAccess.class,ChunkPos.class,CompoundTag.class);
        pack.setAccessible(true); unpack.setAccessible(true); unpackRefs.setAccessible(true);
        var expected = (CompoundTag)pack.invoke(null,context,pos,Map.of(ruin,start),Map.of(ruin,set));
        NbtIo.write(expected,Path.of(args[0]));
        var rust = NbtIo.read(Path.of(args[1]));
        if (!expected.equals(rust)) throw new AssertionError("Rust differs: " + rust + " expected=" + expected);
        var starts = (Map<?,?>)unpack.invoke(null,context,rust,42L);
        var refs = (Map<?,?>)unpackRefs.invoke(null,access,pos,rust);
        var restored = (StructureStart)starts.get(ruin);
        if (restored == null || restored.getPieces().size() != 2 || refs.size() != 1)
            throw new AssertionError("Ocean start or references missing");
        var roundTrip = (CompoundTag)pack.invoke(null,context,pos,starts,refs);
        if (!rust.equals(roundTrip)) throw new AssertionError("Java reload changed ocean structure");
    }
}
