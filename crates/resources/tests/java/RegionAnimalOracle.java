import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderOwner;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BiomeTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.animal.sheep.SheepColorSpawnRules;
import net.minecraft.world.level.*;
import net.minecraft.world.level.levelgen.*;
import sun.misc.Unsafe;

public final class RegionAnimalOracle {
    static class Animal extends AgeableMob {
        int age = 123;
        boolean left;
        AttributeInstance follow;
        Animal() { super(null, null); }
        @Override public AgeableMob getBreedOffspring(ServerLevel level, AgeableMob other) { return null; }
        @Override public void setAge(int value) { age = value; }
        @Override public void setLeftHanded(boolean value) { left = value; }
        @Override public AttributeInstance getAttribute(Holder<Attribute> attribute) { return follow; }
        static Identifier bonusId() { return RANDOM_SPAWN_BONUS_ID; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (Unsafe)field.get(null);
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var element : input) {
            var test = element.getAsJsonObject();
            long seed = test.get("seed").getAsLong();
            RandomSource root = test.get("legacy").getAsBoolean() ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
            var factory = root.forkPositional().fromHashOf(Identifier.withDefaultNamespace("worldgen_region_random")).forkPositional();
            var random = factory.at(test.get("x").getAsInt() << 4, 0, test.get("z").getAsInt() << 4);
            var world = (ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class},
                (proxy, method, values) -> { if (method.getName().equals("getRandom")) return random; throw new AssertionError(method); });
            var biome = Holder.Reference.createStandAlone(new HolderOwner<net.minecraft.world.level.biome.Biome>() {}, net.minecraft.world.level.biome.Biomes.PLAINS);
            var tags = new ArrayList<net.minecraft.tags.TagKey<net.minecraft.world.level.biome.Biome>>();
            if (test.get("warm").getAsBoolean()) tags.add(BiomeTags.SPAWNS_WARM_VARIANT_FARM_ANIMALS);
            if (test.get("cold").getAsBoolean()) tags.add(BiomeTags.SPAWNS_COLD_VARIANT_FARM_ANIMALS);
            var bindTags = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
            bindTags.setAccessible(true);
            bindTags.invoke(biome, tags);
            var group = new AgeableMob.AgeableMobGroupData(test.get("baby").getAsBoolean(), test.get("chance").getAsFloat());
            for (int i = 0; i < test.get("size").getAsInt(); i++) group.increaseGroupSizeByOne();
            var animals = new JsonArray();
            for (int i = 0; i < 3; i++) {
                var color = SheepColorSpawnRules.getSheepColor(biome, random);
                var animal = (Animal)unsafe.allocateInstance(Animal.class); animal.age = 123;
                animal.follow = new AttributeInstance(Attributes.FOLLOW_RANGE, ignored -> {});
                if (test.get("existing").getAsBoolean()) animal.follow.addTransientModifier(new AttributeModifier(Animal.bonusId(), 0.25, AttributeModifier.Operation.ADD_MULTIPLIED_BASE));
                var result = animal.finalizeSpawn(world, null, EntitySpawnReason.CHUNK_GENERATION, group);
                if (result != group) throw new AssertionError("group identity changed");
                var modifier = animal.follow.getModifier(Animal.bonusId());
                var value = new JsonObject();
                value.addProperty("color", color.getSerializedName());
                value.addProperty("age", animal.age);
                value.addProperty("size", group.getGroupSize());
                value.addProperty("left", animal.left);
                value.addProperty("modifier_bits", Double.doubleToRawLongBits(modifier.amount()));
                value.addProperty("permanent", animal.follow.getPermanentModifiers().contains(modifier));
                animals.add(value);
            }
            var value = new JsonObject();
            value.add("animals", animals);
            value.addProperty("next", random.nextLong());
            output.add(value);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
