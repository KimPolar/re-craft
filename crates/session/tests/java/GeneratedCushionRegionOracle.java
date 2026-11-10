import java.io.DataInput;
import java.io.DataOutput;
import java.nio.file.Path;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.decoration.Cushion;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.storage.RegionFile;
import net.minecraft.world.level.chunk.storage.RegionFileVersion;
import net.minecraft.world.level.chunk.storage.RegionStorageInfo;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.TagValueOutput;

/** Read and rewrite a disposable generated entity region using Java 26.3. */
public class GeneratedCushionRegionOracle {
    static class World extends ServerLevel {
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public int getNextEntityId() { return 1; }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var world = (World) ((sun.misc.Unsafe) field.get(null)).allocateInstance(World.class);
        var lookup = VanillaRegistries.createWorldLookup();
        var directory = Path.of(args[0]);
        var pos = new ChunkPos(Integer.parseInt(args[1]), Integer.parseInt(args[2]));
        var path = directory.resolve("r." + Math.floorDiv(pos.x(), 32) + "." + Math.floorDiv(pos.z(), 32) + ".mca");
        try (var region = new RegionFile(new RegionStorageInfo("verification", null, "entities"),
                path, directory, RegionFileVersion.VERSION_DEFLATE, false)) {
            CompoundTag chunk;
            try (var input = region.getChunkDataInputStream(pos)) {
                if (input == null) throw new AssertionError("entity chunk missing");
                chunk = NbtIo.read((DataInput) input);
            }
            if (chunk.getIntOr("DataVersion", 0) != 5023 ||
                    !chunk.read("Position", ChunkPos.CODEC).orElseThrow().equals(pos))
                throw new AssertionError("entity chunk header differs");
            var entities = chunk.getListOrEmpty("Entities");
            if (entities.size() != 1 || !(entities.get(0) instanceof CompoundTag saved) ||
                    !saved.getStringOr("id", "").equals("minecraft:cushion"))
                throw new AssertionError("cushion entity missing");
            var cushion = new Cushion(EntityTypes.CUSHION, world);
            cushion.load(TagValueInput.create(ProblemReporter.DISCARDING, lookup, saved));
            var output = TagValueOutput.createWithContext(ProblemReporter.DISCARDING, lookup);
            if (!cushion.save(output)) throw new AssertionError("cushion was not saved");
            var rewritten = output.buildResult();
            if (!saved.equals(rewritten)) throw new AssertionError("cushion NBT changed");
            entities.set(0, rewritten);
            chunk.put("Entities", entities);
            try (var outputStream = region.getChunkDataOutputStream(pos)) {
                NbtIo.write(chunk, (DataOutput) outputStream);
            }
        }
    }
}
