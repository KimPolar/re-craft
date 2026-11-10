import java.nio.file.Path;
import java.util.*;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.*;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.npc.villager.Villager;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.storage.*;
import net.minecraft.world.phys.Vec3;

/** Run the Java 26.3 village-template load and STRUCTURE finalization sequence. */
public class GeneratedVillageVillagerOracle {
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
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        environment = net.minecraft.world.attribute.EnvironmentAttributeSystem.builder().build();
        access = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true); unsafe = (sun.misc.Unsafe) field.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        long[] seeds = {0L, 38L, 8L, -1L};
        int count = 0;
        try (var jar = new ZipFile(args[0])) {
            var entries = jar.entries();
            while (entries.hasMoreElements()) {
                var entry = entries.nextElement();
                if (!entry.getName().startsWith("data/minecraft/structure/village/") || !entry.getName().endsWith(".nbt")) continue;
                try (var input = jar.getInputStream(entry)) {
                    var template = NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap());
                    for (var tag : template.getListOrEmpty("entities")) {
                        var source = (CompoundTag) tag;
                        if (!source.getCompoundOrEmpty("nbt").getStringOr("id", "").equals("minecraft:villager")) continue;
                        if (template.getIntOr("DataVersion", -1) != 5023) throw new AssertionError("wrong data version");
                        NbtIo.write(source, Path.of(args[1]).resolve("source-" + count + ".nbt"));
                        var local = source.read("pos", Vec3.CODEC).orElseThrow();
                        var origin = new BlockPos(-32, 80, 48);
                        for (var rotation : Rotation.values()) for (int algorithm = 0; algorithm < 2; algorithm++) for (int seed = 0; seed < seeds.length; seed++) {
                            var point = StructureTemplate.transform(local, Mirror.NONE, rotation, BlockPos.ZERO).add(Vec3.atLowerCornerOf(origin));
                            var nbt = source.getCompoundOrEmpty("nbt").copy();
                            nbt.store("Pos", Vec3.CODEC, point); nbt.remove("UUID");
                            var problems = new ProblemReporter.Collector();
                            var villager = new TestVillager(world);
                            villager.load(TagValueInput.create(problems, access, nbt));
                            if (!problems.isEmpty()) throw new AssertionError(problems.getReport());
                            villager.setUUID(new UUID(1, 2));
                            float yaw = villager.rotate(rotation) + (villager.mirror(Mirror.NONE) - villager.getYRot());
                            villager.snapTo(point.x, point.y, point.z, yaw, villager.getXRot());
                            villager.setYBodyRot(yaw); villager.setYHeadRot(yaw);
                            RandomSource random = algorithm == 0 ? new LegacyRandomSource(seeds[seed]) : new XoroshiroRandomSource(seeds[seed]);
                            var region = (ServerLevelAccessor) java.lang.reflect.Proxy.newProxyInstance(
                                ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class},
                                (p, method, values) -> switch (method.getName()) {
                                    case "getRandom" -> random;
                                    case "registryAccess" -> access;
                                    default -> throw new AssertionError(method);
                                });
                            villager.finalizeSpawn(region, null, EntitySpawnReason.STRUCTURE, null);
                            var output = TagValueOutput.createWithContext(problems, access);
                            if (!villager.save(output) || !problems.isEmpty()) throw new AssertionError(problems.getReport());
                            NbtIo.write(output.buildResult(), Path.of(args[1]).resolve("saved-" + count + "-" + rotation.ordinal() + "-" + algorithm + "-" + seed + ".nbt"));
                            out.println("CASE " + count + " " + rotation.ordinal() + " " + algorithm + " " + seed + " " + random.nextLong());
                        }
                        count++;
                    }
                }
            }
        }
        out.println("VILLAGERS " + count);
    }
}
