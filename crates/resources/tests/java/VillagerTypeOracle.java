import com.mojang.serialization.Lifecycle;
import java.util.List;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.*;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.npc.villager.*;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.storage.TagValueInput;

/** Exercise Java's biome mapping and actual VillagerDataHolder finalization. */
public class VillagerTypeOracle {
    static RegistryAccess access;
    static net.minecraft.world.attribute.EnvironmentAttributeSystem environment;
    static sun.misc.Unsafe unsafe;
    static class DummyNavigation extends net.minecraft.world.entity.ai.navigation.GroundPathNavigation {
        DummyNavigation() { super(null, null); }
        @Override public void setCanFloat(boolean value) {}
        @Override public void setCanOpenDoors(boolean value) {}
        @Override public void setRequiredPathLength(float value) {}
    }
    static class World extends ServerLevel {
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public int getNextEntityId() { return 1; }
        @Override public RegistryAccess registryAccess() { return access; }
        @Override public long getGameTime() { return 0L; }
        @Override public net.minecraft.world.attribute.EnvironmentAttributeSystem environmentAttributes() { return environment; }
    }
    static class TestVillager extends Villager {
        TestVillager(World world) { super(EntityTypes.VILLAGER, world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) {
            try { return (DummyNavigation) unsafe.allocateInstance(DummyNavigation.class); }
            catch (InstantiationException error) { throw new AssertionError(error); }
        }
        @Override protected void registerGoals() {}
        @Override public void refreshBrain(ServerLevel level) {}
    }
    static class Identity implements VillagerDataHolder {
        VillagerData data;
        boolean finalized;
        Identity(VillagerData data, boolean finalized) { this.data = data; this.finalized = finalized; }
        public VillagerData getVillagerData() { return data; }
        public void setVillagerData(VillagerData data) { this.data = data; }
        public boolean getVillagerDataFinalized() { return finalized; }
        public void setVillagerDataFinalized(boolean value) { finalized = value; }
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        environment = net.minecraft.world.attribute.EnvironmentAttributeSystem.builder().build();
        var lookup = VanillaRegistries.createWorldLookup();
        var biomes = lookup.lookupOrThrow(Registries.BIOME);
        access = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        var defaults = new VillagerData(access.getOrThrow(VillagerData.DEFAULT_TYPE),
            access.getOrThrow(VillagerProfession.NONE), 1);
        var all = biomes.listElements().toList();
        for (var biome : all) {
            var selected = VillagerType.byBiome(biome).identifier().toString();
            out.println("BIOME " + biome.key().identifier() + " " + selected);
            for (boolean finalized : new boolean[]{false, true}) {
                var identity = new Identity(defaults, finalized);
                var calls = new int[1];
                var region = (ServerLevelAccessor) java.lang.reflect.Proxy.newProxyInstance(
                    ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class},
                    (p, method, arguments) -> switch (method.getName()) {
                        case "getBiome" -> { calls[0]++; yield biome; }
                        case "registryAccess" -> access;
                        default -> throw new AssertionError(method);
                    });
                identity.finalizeVillagerType(region, BlockPos.ZERO);
                out.println("FINAL " + biome.key().identifier() + " " + finalized + " " +
                    identity.data.type().unwrapKey().orElseThrow().identifier() + " " + identity.finalized + " " + calls[0]);
            }
        }
        var example = all.get(0).value();
        for (String name : List.of("example:desert", "minecraft:unknown_test")) {
            var registry = new MappedRegistry<Biome>(Registries.BIOME, Lifecycle.stable());
            var key = ResourceKey.create(Registries.BIOME, Identifier.parse(name));
            var holder = registry.register(key, example, RegistrationInfo.BUILT_IN);
            out.println("EXTRA " + name + " " + VillagerType.byBiome(holder).identifier());
        }
        out.println("DIRECT " + VillagerType.byBiome(Holder.direct(example)).identifier());
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        var biome = all.get(0);
        try (var jar = new ZipFile(args[0])) {
            var entries = jar.entries();
            while (entries.hasMoreElements()) {
                var entry = entries.nextElement();
                if (!entry.getName().startsWith("data/minecraft/structure/village/") || !entry.getName().endsWith(".nbt")) continue;
                try (var input = jar.getInputStream(entry)) {
                    var template = NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap());
                    for (var tag : template.getListOrEmpty("entities")) {
                        var source = ((CompoundTag) tag).getCompoundOrEmpty("nbt");
                        if (!source.getStringOr("id", "").equals("minecraft:villager")) continue;
                        var data = source.get("VillagerData");
                        boolean decoded = data != null && VillagerData.CODEC.parse(access.createSerializationContext(NbtOps.INSTANCE), data).result().isPresent();
                        boolean stored = source.getBooleanOr("VillagerDataFinalized", false);
                        var problems = new ProblemReporter.Collector();
                        var villager = new TestVillager(world);
                        villager.load(TagValueInput.create(problems, access, source));
                        if (!problems.isEmpty()) throw new AssertionError(problems.getReport());
                        boolean actual = villager.getVillagerDataFinalized();
                        String before = villager.getVillagerData().type().unwrapKey().orElseThrow().identifier().toString();
                        var calls = new int[1];
                        var region = (ServerLevelAccessor) java.lang.reflect.Proxy.newProxyInstance(
                            ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class},
                            (p, method, arguments) -> switch (method.getName()) {
                                case "getBiome" -> { calls[0]++; yield biome; }
                                case "registryAccess" -> access;
                                default -> throw new AssertionError(method);
                            });
                        villager.finalizeVillagerType(region, BlockPos.ZERO);
                        out.println("LOAD " + entry.getName() + " " + stored + " " + decoded + " " + actual + " " + before + " " +
                            villager.getVillagerData().type().unwrapKey().orElseThrow().identifier() + " " + calls[0]);
                    }
                }
            }
        }
    }
}
