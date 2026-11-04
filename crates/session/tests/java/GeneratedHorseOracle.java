import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.*;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.ai.attributes.Attributes;
import net.minecraft.world.entity.animal.equine.Horse;
import net.minecraft.world.level.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.storage.TagValueOutput;
import net.minecraft.resources.Identifier;
import sun.misc.Unsafe;

public class GeneratedHorseOracle {
    static RegistryAccess access;
    static Unsafe unsafe;
    static class DummyNavigation extends net.minecraft.world.entity.ai.navigation.GroundPathNavigation {
        DummyNavigation() { super(null,null); }
        @Override public void setCanFloat(boolean value) {}
    }
    static class World extends ServerLevel {
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public int getNextEntityId() { return 1; }
        @Override public RegistryAccess registryAccess() { return access; }
    }
    static class TestHorse extends Horse {
        TestHorse(World world) { super(EntityTypes.HORSE,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return null; }
        @Override protected void registerGoals() {}
    }
    static class ObserveHorse extends Horse {
        ObserveHorse(World world) { super(EntityTypes.HORSE,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) {
            try { return (DummyNavigation)unsafe.allocateInstance(DummyNavigation.class); }
            catch (InstantiationException error) { throw new AssertionError(error); }
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        access = new RegistryAccess.ImmutableRegistryAccess(List.of());
        var field = Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        unsafe = (Unsafe)field.get(null);
        var world = (World)unsafe.allocateInstance(World.class);
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var item : cases) {
            var test = item.getAsJsonObject();
            long seed = test.get("seed").getAsLong();
            RandomSource root = test.get("legacy").getAsBoolean() ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
            var random = root.forkPositional().fromHashOf(Identifier.withDefaultNamespace("worldgen_region_random")).forkPositional().at(-13 << 4,0,17 << 4);
            var region = (ServerLevelAccessor)java.lang.reflect.Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)-> {
                return switch (m.getName()) {
                    case "getRandom" -> random;
                    case "registryAccess" -> access;
                    default -> throw new AssertionError(m);
                };
            });
            SpawnGroupData group = null;
            var animals = new JsonArray();
            for (int i=0; i<6; i++) {
                var horse = new TestHorse(world);
                horse.setUUID(new UUID(1,2)); horse.setPos(3.5,10,-2.5); horse.setYRot(22.5f); horse.setXRot(0);
                group = horse.finalizeSpawn(region,null,EntitySpawnReason.CHUNK_GENERATION,group);
                var tagOut = TagValueOutput.createWithContext(ProblemReporter.DISCARDING,lookup);
                if (!horse.save(tagOut)) throw new AssertionError("entity not saved");
                var javaTag = tagOut.buildResult();
                if (test.has("paths")) {
                    var rustTag = NbtIo.read(Path.of(test.getAsJsonArray("paths").get(i).getAsString()));
                    var order = Comparator.comparing((net.minecraft.nbt.Tag tag) -> ((net.minecraft.nbt.CompoundTag)tag).getString("id").orElseThrow());
                    javaTag.getList("attributes").orElseThrow().sort(order);
                    rustTag.getList("attributes").orElseThrow().sort(order);
                    if (!javaTag.equals(rustTag)) throw new AssertionError("seed="+seed+" index="+i+" java="+javaTag+" rust="+rustTag);
                }
                var result = new JsonObject();
                result.addProperty("variant",horse.getVariant().getId());
                result.addProperty("markings",horse.getMarkings().getId());
                result.addProperty("health_bits",Float.floatToRawIntBits((float)horse.getAttributeValue(Attributes.MAX_HEALTH)));
                result.addProperty("speed_bits",Double.doubleToRawLongBits(horse.getAttributeValue(Attributes.MOVEMENT_SPEED)));
                result.addProperty("jump_bits",Double.doubleToRawLongBits(horse.getAttributeValue(Attributes.JUMP_STRENGTH)));
                result.addProperty("baby",horse.isBaby());
                result.addProperty("left",horse.isLeftHanded());
                result.addProperty("nbt",javaTag.toString());
                animals.add(result);
            }
            var result = new JsonObject(); result.add("animals",animals); result.addProperty("next",random.nextLong()); output.add(result);
        }
        Files.writeString(Path.of(args[1]),output.toString());
        if (args.length > 2) {
            var seedField = LegacyRandomSource.class.getDeclaredField("seed"); seedField.setAccessible(true);
            var constructors = new JsonArray();
            for (int i=0; i<32; i++) {
                var horse = new ObserveHorse(world);
                var state = ((java.util.concurrent.atomic.AtomicLong)seedField.get(horse.getRandom())).get();
                var uuid = horse.getUUID(); var value = new JsonObject(); var words = new JsonArray();
                words.add((int)(uuid.getMostSignificantBits()>>>32)); words.add((int)uuid.getMostSignificantBits());
                words.add((int)(uuid.getLeastSignificantBits()>>>32)); words.add((int)uuid.getLeastSignificantBits());
                value.add("uuid",words); value.addProperty("state",state);
                value.addProperty("yaw_bits",Float.floatToRawIntBits(horse.getYRot())); constructors.add(value);
            }
            Files.writeString(Path.of(args[2]),constructors.toString());
        }
    }
}
