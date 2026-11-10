import java.io.DataInput;
import java.io.DataOutput;
import java.nio.file.Path;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.npc.villager.Villager;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.chunk.storage.RegionFile;
import net.minecraft.world.level.chunk.storage.RegionFileVersion;
import net.minecraft.world.level.chunk.storage.RegionStorageInfo;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.TagValueOutput;

/** Read and rewrite a disposable entity region using Java 26.3. */
public class GeneratedVillagerRegionOracle {
    static RegistryAccess access;
    static net.minecraft.world.attribute.EnvironmentAttributeSystem environment;
    static sun.misc.Unsafe unsafe;
    static class Navigation extends net.minecraft.world.entity.ai.navigation.GroundPathNavigation {
        Navigation() { super(null, null); }
        @Override public void setCanFloat(boolean value) {}
        @Override public void setCanOpenDoors(boolean value) {}
        @Override public void setRequiredPathLength(float value) {}
    }
    static class World extends ServerLevel {
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public int getNextEntityId() { return 1; }
        @Override public RegistryAccess registryAccess() { return access; }
        @Override public long getGameTime() { return 0; }
        @Override public net.minecraft.world.attribute.EnvironmentAttributeSystem environmentAttributes() { return environment; }
    }
    static class TestVillager extends Villager {
        TestVillager(World world) { super(EntityTypes.VILLAGER, world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) {
            try { return (Navigation) unsafe.allocateInstance(Navigation.class); }
            catch (InstantiationException error) { throw new AssertionError(error); }
        }
        @Override protected void registerGoals() {}
        @Override public void refreshBrain(ServerLevel level) {}
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        access = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        environment = net.minecraft.world.attribute.EnvironmentAttributeSystem.builder().build();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true); unsafe = (sun.misc.Unsafe) field.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
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
                    !saved.getStringOr("id", "").equals("minecraft:villager"))
                throw new AssertionError("villager entity missing");
            var problems = new ProblemReporter.Collector();
            var villager = new TestVillager(world);
            villager.load(TagValueInput.create(problems, access, saved));
            if (!problems.isEmpty()) throw new AssertionError(problems.getReport());
            var output = TagValueOutput.createWithContext(problems, access);
            if (!villager.save(output) || !problems.isEmpty()) throw new AssertionError(problems.getReport());
            var rewritten = output.buildResult();
            if (!saved.equals(rewritten)) throw new AssertionError("villager NBT changed: " + saved + " != " + rewritten);
            entities.set(0, rewritten);
            chunk.put("Entities", entities);
            try (var outputStream = region.getChunkDataOutputStream(pos)) {
                NbtIo.write(chunk, (DataOutput) outputStream);
            }
        }
    }
}
