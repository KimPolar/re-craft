import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BiomeTags;
import net.minecraft.util.*;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.animal.pig.Pig;
import net.minecraft.world.entity.animal.cow.Cow;
import net.minecraft.world.entity.animal.chicken.Chicken;
import net.minecraft.world.entity.animal.sheep.Sheep;
import net.minecraft.world.entity.animal.nautilus.ZombieNautilus;
import net.minecraft.world.level.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.storage.TagValueOutput;
import sun.misc.Unsafe;

public class GeneratedFarmAnimalOracle {
    static RegistryAccess access;
    static Unsafe unsafe;
    static class DummyNavigation extends net.minecraft.world.entity.ai.navigation.GroundPathNavigation {
        DummyNavigation() { super(null,null); }
        @Override public void setCanFloat(boolean value) {}
    }
    static net.minecraft.world.entity.ai.navigation.PathNavigation dummyNavigation() {
        try { return (DummyNavigation)unsafe.allocateInstance(DummyNavigation.class); }
        catch (InstantiationException error) { throw new AssertionError(error); }
    }
    static class World extends ServerLevel {
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public int getNextEntityId() { return 1; }
        @Override public RegistryAccess registryAccess() { return access; }
    }
    static class TestPig extends Pig {
        TestPig(World world) { super(EntityTypes.PIG,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return null; }
        @Override protected void registerGoals() {}
    }
    static class TestCow extends Cow {
        TestCow(World world) { super(EntityTypes.COW,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return null; }
        @Override protected void registerGoals() {}
    }
    static class TestChicken extends Chicken {
        TestChicken(World world) { super(EntityTypes.CHICKEN,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return null; }
        @Override protected void registerGoals() {}
    }
    static class ObserveSheep extends Sheep {
        ObserveSheep(World world) { super(EntityTypes.SHEEP,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return dummyNavigation(); }
    }
    static class ObservePig extends Pig {
        ObservePig(World world) { super(EntityTypes.PIG,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return dummyNavigation(); }
    }
    static class ObserveCow extends Cow {
        ObserveCow(World world) { super(EntityTypes.COW,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return dummyNavigation(); }
    }
    static class ObserveChicken extends Chicken {
        ObserveChicken(World world) { super(EntityTypes.CHICKEN,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return dummyNavigation(); }
    }
    static class ObserveZombieNautilus extends ZombieNautilus {
        ObserveZombieNautilus(World world) { super(EntityTypes.ZOMBIE_NAUTILUS,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return null; }
    }
    static Identifier fileKey(Identifier id) {
        return Identifier.fromNamespaceAndPath(id.getNamespace(), "entry/" + id.getPath() + ".json");
    }
    static <T> Registry<T> registry(ResourceKey<? extends Registry<T>> key, List<Holder.Reference<T>> values) {
        var registry = new MappedRegistry<T>(key,com.mojang.serialization.Lifecycle.stable());
        values.stream().sorted(Comparator.comparing(h -> fileKey(h.key().identifier())))
            .forEach(h -> registry.register(h.key(),h.value(),RegistrationInfo.BUILT_IN));
        return registry.freeze();
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup=VanillaRegistries.createWorldLookup();
        access=new RegistryAccess.ImmutableRegistryAccess(List.of(
            registry(Registries.PIG_VARIANT,lookup.lookupOrThrow(Registries.PIG_VARIANT).listElements().toList()),
            registry(Registries.COW_VARIANT,lookup.lookupOrThrow(Registries.COW_VARIANT).listElements().toList()),
            registry(Registries.CHICKEN_VARIANT,lookup.lookupOrThrow(Registries.CHICKEN_VARIANT).listElements().toList()),
            registry(Registries.PIG_SOUND_VARIANT,lookup.lookupOrThrow(Registries.PIG_SOUND_VARIANT).listElements().toList()),
            registry(Registries.COW_SOUND_VARIANT,lookup.lookupOrThrow(Registries.COW_SOUND_VARIANT).listElements().toList()),
            registry(Registries.CHICKEN_SOUND_VARIANT,lookup.lookupOrThrow(Registries.CHICKEN_SOUND_VARIANT).listElements().toList()),
            registry(Registries.ZOMBIE_NAUTILUS_VARIANT,lookup.lookupOrThrow(Registries.ZOMBIE_NAUTILUS_VARIANT).listElements().toList())
        ));
        var unsafeField=Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        unsafe=(Unsafe)unsafeField.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var eggField=Chicken.class.getDeclaredField("eggTime"); eggField.setAccessible(true);
        var bindTags=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bindTags.setAccessible(true);
        var input=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        for(var entry:input) {
            var test=entry.getAsJsonObject(); long seed=test.get("seed").getAsLong();
            RandomSource root=test.get("legacy").getAsBoolean() ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
            var random=root.forkPositional().fromHashOf(Identifier.withDefaultNamespace("worldgen_region_random")).forkPositional().at(-13<<4,0,17<<4);
            var biomeId=Identifier.parse(test.get("biome").getAsString());
            var biome=lookup.lookupOrThrow(Registries.BIOME).getOrThrow(ResourceKey.create(Registries.BIOME,biomeId));
            var tags=new ArrayList<net.minecraft.tags.TagKey<net.minecraft.world.level.biome.Biome>>();
            switch(biomeId.toString()) {
                case "minecraft:desert" -> tags.add(BiomeTags.SPAWNS_WARM_VARIANT_FARM_ANIMALS);
                case "minecraft:snowy_plains" -> tags.add(BiomeTags.SPAWNS_COLD_VARIANT_FARM_ANIMALS);
                case "minecraft:plains" -> { }
                default -> throw new AssertionError(biomeId);
            }
            bindTags.invoke(biome,tags);
            var region=(ServerLevelAccessor)java.lang.reflect.Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)-> {
                return switch(m.getName()) {
                    case "getRandom" -> random;
                    case "getBiome" -> biome;
                    case "registryAccess" -> access;
                    case "environmentAttributes" -> null;
                    default -> throw new AssertionError(m);
                };
            });
            var group=new AgeableMob.AgeableMobGroupData(true,test.get("chance").getAsFloat());
            boolean jockey=test.has("jockey") && test.get("jockey").getAsBoolean();
            int index=0;
            for(var path:test.getAsJsonArray("paths")) {
                var animal=switch(test.get("animal").getAsString()) {
                    case "pig" -> new TestPig(world);
                    case "cow" -> new TestCow(world);
                    case "chicken" -> new TestChicken(world);
                    default -> throw new AssertionError();
                };
                if(animal instanceof Chicken chicken) eggField.setInt(chicken,new int[]{6000,8999,11999}[index]);
                animal.setUUID(new UUID(1,2)); animal.setPos(3.5,10,-2.5); animal.setYRot(22.5f); animal.setXRot(0);
                var result=animal.finalizeSpawn(region,null,jockey?EntitySpawnReason.JOCKEY:EntitySpawnReason.CHUNK_GENERATION,jockey?null:group);
                if(!jockey && result!=group) throw new AssertionError("group identity");
                if(jockey) ((Chicken)animal).setChickenJockey(true);
                var output=TagValueOutput.createWithContext(ProblemReporter.DISCARDING,lookup);
                if(!animal.save(output)) throw new AssertionError("entity not saved");
                var javaTag=output.buildResult(); var rustTag=NbtIo.read(Path.of(path.getAsString()));
                if(!javaTag.equals(rustTag)) throw new AssertionError("case="+test+" index="+index+" java="+javaTag+" rust="+rustTag);
                index++;
            }
            if(jockey && random.nextLong()!=test.get("next_random").getAsLong()) throw new AssertionError("jockey random mismatch: "+test);
        }
        if(args.length>1) {
            var seedField=LegacyRandomSource.class.getDeclaredField("seed"); seedField.setAccessible(true);
            var sensorsField=net.minecraft.world.entity.ai.Brain.class.getDeclaredField("sensors"); sensorsField.setAccessible(true);
            var delayField=net.minecraft.world.entity.ai.sensing.Sensor.class.getDeclaredField("timeToTick"); delayField.setAccessible(true);
            var results=new JsonArray();
            for(var type:List.of("sheep","pig","cow","chicken","zombie_nautilus")) for(int i=0;i<32;i++) {
                var animal=switch(type) {
                    case "sheep" -> new ObserveSheep(world);
                    case "pig" -> new ObservePig(world);
                    case "cow" -> new ObserveCow(world);
                    case "chicken" -> new ObserveChicken(world);
                    case "zombie_nautilus" -> new ObserveZombieNautilus(world);
                    default -> throw new AssertionError();
                };
                var state=((java.util.concurrent.atomic.AtomicLong)seedField.get(animal.getRandom())).get();
                var uuid=animal.getUUID(); var value=new JsonObject();
                var words=new JsonArray(); words.add((int)(uuid.getMostSignificantBits()>>>32)); words.add((int)uuid.getMostSignificantBits());
                words.add((int)(uuid.getLeastSignificantBits()>>>32)); words.add((int)uuid.getLeastSignificantBits());
                value.addProperty("entity",type); value.add("uuid",words); value.addProperty("state",state);
                value.addProperty("yaw_bits",Float.floatToRawIntBits(animal.getYRot()));
                if(animal instanceof ZombieNautilus) {
                    var sensors=(Map<?,?>)sensorsField.get(animal.getBrain());
                    var delays=new JsonArray();
                    for(var sensor:List.of(net.minecraft.world.entity.ai.sensing.SensorType.NEAREST_LIVING_ENTITIES,
                            net.minecraft.world.entity.ai.sensing.SensorType.NEAREST_ADULT,
                            net.minecraft.world.entity.ai.sensing.SensorType.NEAREST_PLAYERS,
                            net.minecraft.world.entity.ai.sensing.SensorType.HURT_BY,
                            net.minecraft.world.entity.ai.sensing.SensorType.NAUTILUS_TEMPTATIONS))
                        delays.add(delayField.getLong(sensors.get(sensor)));
                    value.add("sensor_delays",delays);
                }
                if(animal instanceof Chicken chicken) value.addProperty("egg",eggField.getInt(chicken));
                else value.add("egg",JsonNull.INSTANCE);
                results.add(value);
            }
            Files.writeString(Path.of(args[1]),results.toString());
        }
    }
}
