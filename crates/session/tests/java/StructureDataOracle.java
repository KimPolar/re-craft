import com.mojang.serialization.Lifecycle;
import it.unimi.dsi.fastutil.longs.LongOpenHashSet;
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
import net.minecraft.world.level.levelgen.structure.structures.BuriedTreasurePieces.BuriedTreasurePiece;
import net.minecraft.world.level.levelgen.structure.structures.SwampHutPiece;

public class StructureDataOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        Structure structure = lookup.lookupOrThrow(Registries.STRUCTURE)
                .getOrThrow(ResourceKey.create(Registries.STRUCTURE, Identifier.parse("minecraft:buried_treasure"))).value();
        Structure hut = lookup.lookupOrThrow(Registries.STRUCTURE)
                .getOrThrow(ResourceKey.create(Registries.STRUCTURE, Identifier.parse("minecraft:swamp_hut"))).value();
        var registry = new MappedRegistry<Structure>(Registries.STRUCTURE, Lifecycle.stable());
        Registry.register(registry, Identifier.parse("example:treasure"), structure);
        Registry.register(registry, Identifier.parse("example:hut"), hut);
        registry.freeze();
        var access = new RegistryAccess.ImmutableRegistryAccess(List.of(registry));
        var context = new StructurePieceSerializationContext(null, access, null);
        if (args.length > 1 && args[1].equals("verify_hut")) {
            var readStarts = SerializableChunkData.class.getDeclaredMethod("unpackStructureStart",
                    StructurePieceSerializationContext.class, net.minecraft.nbt.CompoundTag.class, long.class);
            var readReferences = SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences",
                    RegistryAccess.class, ChunkPos.class, net.minecraft.nbt.CompoundTag.class);
            var pack = SerializableChunkData.class.getDeclaredMethod("packStructureData",
                    StructurePieceSerializationContext.class, ChunkPos.class, Map.class, Map.class);
            readStarts.setAccessible(true); readReferences.setAccessible(true); pack.setAccessible(true);
            var pos = new ChunkPos(-17, 23);
            for (int i=0; i<16; i++) {
                var tag = NbtIo.read(Path.of(args[0], "hut-rust-"+i+".nbt"));
                var starts = (Map<?, ?>)readStarts.invoke(null, context, tag, 42L);
                var references = (Map<?, ?>)readReferences.invoke(null, access, pos, tag);
                var start = (StructureStart)starts.get(hut);
                if (starts.size()!=1 || start==null || !start.getChunkPos().equals(pos)
                        || start.getPieces().size()!=1 || !(start.getPieces().get(0) instanceof SwampHutPiece))
                    throw new AssertionError("invalid hut start " + i);
                var set = (it.unimi.dsi.fastutil.longs.LongSet)references.get(hut);
                if (references.size()!=1 || set==null || set.size()!=1 || !set.contains(ChunkPos.pack(pos.x(), pos.z())))
                    throw new AssertionError("invalid hut references " + i);
                var repacked = (net.minecraft.nbt.CompoundTag)pack.invoke(null, context, pos, starts, references);
                if (!tag.equals(repacked)) throw new AssertionError("hut changed in Java: " + i + " input=" + tag + " output=" + repacked);
            }
            return;
        }
        if (args.length > 1 && args[1].equals("verify")) {
            var readStarts = SerializableChunkData.class.getDeclaredMethod("unpackStructureStart",
                    StructurePieceSerializationContext.class, net.minecraft.nbt.CompoundTag.class, long.class);
            var readReferences = SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences",
                    RegistryAccess.class, ChunkPos.class, net.minecraft.nbt.CompoundTag.class);
            readStarts.setAccessible(true);
            readReferences.setAccessible(true);
            for (int i=0; i<3; i++) {
                var pos = i == 2 ? new ChunkPos(-17, 23) : new ChunkPos(0, 0);
                var tag = NbtIo.read(Path.of(args[0], "structure-rust-"+i+".nbt"));
                var starts = (Map<?, ?>)readStarts.invoke(null, context, tag, 42L);
                var references = (Map<?, ?>)readReferences.invoke(null, access, pos, tag);
                if (i == 0) {
                    if (!starts.isEmpty() || !references.isEmpty()) throw new AssertionError("empty structures");
                    continue;
                }
                var start = (StructureStart)starts.get(structure);
                if (starts.size()!=1 || start==null || start.getReferences()!=0 || !start.getChunkPos().equals(pos) || start.getPieces().size()!=1)
                    throw new AssertionError("invalid start " + i);
                var box=start.getPieces().get(0).getBoundingBox();
                int x=pos.x()*16+9, y=i==1?90:-27, z=pos.z()*16+9;
                if (box.minX()!=x || box.minY()!=y || box.minZ()!=z || box.maxX()!=x || box.maxY()!=y || box.maxZ()!=z)
                    throw new AssertionError("invalid piece bounds " + i);
                var set=(it.unimi.dsi.fastutil.longs.LongSet)references.get(structure);
                if (references.size()!=1 || set==null || set.size()!=3) throw new AssertionError("invalid references " + i);
                for (var offset : new int[][]{{-1,-1},{0,0},{1,-1}})
                    if (!set.contains(ChunkPos.pack(pos.x()+offset[0],pos.z()+offset[1]))) throw new AssertionError("missing reference " + i);
            }
            return;
        }
        var method = SerializableChunkData.class.getDeclaredMethod("packStructureData",
                StructurePieceSerializationContext.class, ChunkPos.class, Map.class, Map.class);
        method.setAccessible(true);
        for (int i=0; i<3; i++) {
            var pos = i == 2 ? new ChunkPos(-17, 23) : new ChunkPos(0, 0);
            var starts = new java.util.HashMap<Structure, StructureStart>();
            var references = new java.util.HashMap<Structure, it.unimi.dsi.fastutil.longs.LongSet>();
            if (i != 0) {
                var piece = new BuriedTreasurePiece(new BlockPos(pos.x()*16+9, i==1 ? 90 : -27, pos.z()*16+9));
                starts.put(structure, new StructureStart(structure, pos, 0, new PiecesContainer(List.of(piece))));
                var set = new LongOpenHashSet();
                for (var offset : new int[][]{{-1,-1},{0,0},{1,-1}})
                    set.add(ChunkPos.pack(pos.x()+offset[0],pos.z()+offset[1]));
                references.put(structure, set);
            }
            var tag = (net.minecraft.nbt.CompoundTag)method.invoke(null, context, pos, starts, references);
            NbtIo.write(tag, Path.of(args[0], "structure-"+i+".nbt"));
        }
    }
}
