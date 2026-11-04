import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BiomeTags;
import net.minecraft.util.RandomSource;
import net.minecraft.core.*;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.animal.pig.PigSoundVariants;
import net.minecraft.world.entity.animal.cow.CowSoundVariants;
import net.minecraft.world.entity.animal.chicken.ChickenSoundVariants;
import net.minecraft.world.entity.variant.PriorityProvider;
import net.minecraft.world.entity.variant.SpawnContext;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import sun.misc.Unsafe;

public class FarmAnimalVariantOracle {
    static Identifier fileKey(Identifier id) {
        return Identifier.fromNamespaceAndPath(id.getNamespace(), "pig_variant/" + id.getPath() + ".json");
    }
    static <T> Registry<T> soundRegistry(ResourceKey<? extends Registry<T>> key, java.util.List<Holder.Reference<T>> values) {
        var registry = new MappedRegistry<T>(key, com.mojang.serialization.Lifecycle.stable());
        values.stream().sorted(java.util.Comparator.comparing(h -> fileKey(h.key().identifier())))
            .forEach(h -> registry.register(h.key(),h.value(),RegistrationInfo.BUILT_IN));
        return registry.freeze();
    }
    static class Animal extends AgeableMob {
        int age = 123;
        boolean left;
        AttributeInstance follow;
        Animal() { super(null,null); }
        @Override public AgeableMob getBreedOffspring(ServerLevel level, AgeableMob other) { return null; }
        @Override public void setAge(int value) { age = value; }
        @Override public void setLeftHanded(boolean value) { left = value; }
        @Override public AttributeInstance getAttribute(Holder<Attribute> attribute) { return follow; }
        static Identifier bonusId() { return RANDOM_SPAWN_BONUS_ID; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var unsafeField = Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (Unsafe)unsafeField.get(null);
        var sounds = new RegistryAccess.ImmutableRegistryAccess(java.util.List.of(
            soundRegistry(Registries.PIG_SOUND_VARIANT, lookup.lookupOrThrow(Registries.PIG_SOUND_VARIANT).listElements().toList()),
            soundRegistry(Registries.COW_SOUND_VARIANT, lookup.lookupOrThrow(Registries.COW_SOUND_VARIANT).listElements().toList()),
            soundRegistry(Registries.CHICKEN_SOUND_VARIANT, lookup.lookupOrThrow(Registries.CHICKEN_SOUND_VARIANT).listElements().toList())
        ));
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonObject();
        var tables = new JsonObject();
        // ResourceManagerRegistryLoadTask sorts file identifiers before it
        // registers elements; VanillaRegistries uses bootstrap order instead.
        var pig = lookup.lookupOrThrow(Registries.PIG_VARIANT).listElements().sorted(java.util.Comparator.comparing(h -> fileKey(h.key().identifier()))).toList();
        var cow = lookup.lookupOrThrow(Registries.COW_VARIANT).listElements().sorted(java.util.Comparator.comparing(h -> fileKey(h.key().identifier()))).toList();
        var chicken = lookup.lookupOrThrow(Registries.CHICKEN_VARIANT).listElements().sorted(java.util.Comparator.comparing(h -> fileKey(h.key().identifier()))).toList();
        for (var item : java.util.List.of(java.util.Map.entry("pig",pig), java.util.Map.entry("cow",cow), java.util.Map.entry("chicken",chicken))) {
            var ids = new JsonArray();
            for (var holder : item.getValue()) ids.add(holder.key().identifier().toString());
            tables.add(item.getKey(),ids);
        }
        output.add("order",tables);
        var synthetic = new JsonArray();
        java.util.List.of("z:a", "a:b", "a:a", "z:a-foo", "a:a/child", "a:a_foo", "a:a.foo", "a:a-foo")
            .stream().map(Identifier::parse).sorted(java.util.Comparator.comparing(FarmAnimalVariantOracle::fileKey))
            .forEach(id -> synthetic.add(id.toString()));
        output.add("synthetic_order", synthetic);
        var results = new JsonArray();
        for (var raw : cases) {
            var test = raw.getAsJsonObject();
            long seed = test.get("seed").getAsLong();
            RandomSource root = test.get("legacy").getAsBoolean() ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
            var random = root.forkPositional().fromHashOf(Identifier.withDefaultNamespace("worldgen_region_random")).forkPositional().at(-13 << 4,0,17 << 4);
            var biomeId = Identifier.parse(test.get("biome").getAsString());
            var biome = lookup.lookupOrThrow(Registries.BIOME).getOrThrow(ResourceKey.create(Registries.BIOME,biomeId));
            // VanillaRegistries produces unbound holders. Bind the memberships
            // independently for these five biomes from the 26.3 built-in tags.
            var tags = new java.util.ArrayList<net.minecraft.tags.TagKey<net.minecraft.world.level.biome.Biome>>();
            switch (biomeId.toString()) {
                case "minecraft:desert", "minecraft:jungle" -> tags.add(BiomeTags.SPAWNS_WARM_VARIANT_FARM_ANIMALS);
                case "minecraft:snowy_plains", "minecraft:taiga" -> tags.add(BiomeTags.SPAWNS_COLD_VARIANT_FARM_ANIMALS);
                case "minecraft:plains" -> { }
                default -> throw new AssertionError("unknown oracle biome " + biomeId);
            }
            var bindTags = net.minecraft.core.Holder.Reference.class.getDeclaredMethod("bindTags",java.util.Collection.class);
            bindTags.setAccessible(true); bindTags.invoke(biome,tags);
            var context = new SpawnContext(null,null,null,biome);
            var selected = switch(test.get("animal").getAsString()) {
                case "pig" -> PriorityProvider.pick(pig.stream(), h -> h.value(), random, context).map(h -> h.key().identifier().toString());
                case "cow" -> PriorityProvider.pick(cow.stream(), h -> h.value(), random, context).map(h -> h.key().identifier().toString());
                case "chicken" -> PriorityProvider.pick(chicken.stream(), h -> h.value(), random, context).map(h -> h.key().identifier().toString());
                default -> throw new AssertionError();
            };
            var result = new JsonObject();
            if(selected.isPresent()) result.addProperty("selected",selected.get()); else result.add("selected",JsonNull.INSTANCE);
            var sound = switch(test.get("animal").getAsString()) {
                case "pig" -> PigSoundVariants.pickRandomSoundVariant(sounds,random).unwrapKey().orElseThrow().identifier();
                case "cow" -> CowSoundVariants.pickRandomSoundVariant(sounds,random).unwrapKey().orElseThrow().identifier();
                case "chicken" -> ChickenSoundVariants.pickRandomSoundVariant(sounds,random).unwrapKey().orElseThrow().identifier();
                default -> throw new AssertionError();
            };
            result.addProperty("sound",sound.toString());
            var world = (ServerLevelAccessor)java.lang.reflect.Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},
                (p,m,v) -> { if(m.getName().equals("getRandom")) return random; throw new AssertionError(m); });
            var group = new AgeableMob.AgeableMobGroupData(true,0.05f);
            for(int i=0; i<test.get("size").getAsInt(); i++) group.increaseGroupSizeByOne();
            var animal = (Animal)unsafe.allocateInstance(Animal.class); animal.age = 123;
            animal.follow = new AttributeInstance(Attributes.FOLLOW_RANGE, ignored -> {});
            if(test.get("existing").getAsBoolean()) animal.follow.addTransientModifier(new AttributeModifier(Animal.bonusId(),0.25,AttributeModifier.Operation.ADD_MULTIPLIED_BASE));
            if(animal.finalizeSpawn(world,null,EntitySpawnReason.CHUNK_GENERATION,group) != group) throw new AssertionError("group identity changed");
            result.addProperty("baby",animal.age == -24000);
            result.addProperty("size",group.getGroupSize());
            result.addProperty("left",animal.left);
            result.addProperty("bonus_bits",Double.doubleToRawLongBits(animal.follow.getModifier(Animal.bonusId()).amount()));
            result.addProperty("next",random.nextLong()); results.add(result);
        }
        output.add("results",results);
        Files.writeString(Path.of(args[1]),output.toString());
    }
}
