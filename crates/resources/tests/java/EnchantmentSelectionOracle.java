import java.util.*;
import java.util.stream.Collectors;
import java.io.InputStreamReader;
import com.google.gson.JsonParser;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.enchantment.Enchantment;
import net.minecraft.world.item.enchantment.Enchantable;
import net.minecraft.world.item.enchantment.EnchantmentHelper;
import net.minecraft.world.item.enchantment.ItemEnchantments;
import net.minecraft.world.item.enchantment.providers.EnchantmentsByCost;
import net.minecraft.world.item.enchantment.providers.SingleEnchantment;
import net.minecraft.util.valueproviders.*;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

public class EnchantmentSelectionOracle {
    static String id(Holder<Enchantment> holder) {
        return holder.unwrapKey().orElseThrow().identifier().toString();
    }

    static Set<String> tagEntries(String kind, net.minecraft.resources.Identifier id, Set<String> visiting) throws Exception {
        String name = id.toString();
        if (!visiting.add(name)) throw new IllegalStateException("cyclic oracle tag: " + name);
        String path = "data/" + id.getNamespace() + "/tags/" + kind + "/" + id.getPath() + ".json";
        var result = new LinkedHashSet<String>();
        try (var stream = EnchantmentSelectionOracle.class.getClassLoader().getResourceAsStream(path)) {
            if (stream == null) throw new IllegalStateException("missing tag " + path);
            var values = JsonParser.parseReader(new InputStreamReader(stream, java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject().getAsJsonArray("values");
            for (var value : values) {
                String entry = value.isJsonObject() ? value.getAsJsonObject().get("id").getAsString() : value.getAsString();
                if (entry.startsWith("#")) result.addAll(tagEntries(kind, net.minecraft.resources.Identifier.parse(entry.substring(1)), visiting));
                else result.add(net.minecraft.resources.Identifier.parse(entry).toString());
            }
        }
        visiting.remove(name);
        return result;
    }

    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var reloadable = VanillaRegistries.createReloadableLookup(lookup);
        for (var initializer : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) initializer.apply();
        var registry = lookup.lookupOrThrow(Registries.ENCHANTMENT);
        for (int algorithm = 0; algorithm < 3; algorithm++) for (int i = 0; i < 512; i++) {
            long seed = i * 3456789L - 177L;
            RandomSource random = switch (algorithm) {
                case 0 -> new LegacyRandomSource(seed);
                case 1 -> new XoroshiroRandomSource(seed);
                default -> new net.minecraft.world.level.levelgen.WorldgenRandom(new XoroshiroRandomSource(seed));
            };
            long a = Double.doubleToLongBits(random.nextGaussian());
            long between = random.nextLong();
            long b = Double.doubleToLongBits(random.nextGaussian());
            int integer = random.nextInt();
            long c = Double.doubleToLongBits(random.nextGaussian());
            random.setSeed(seed + 1);
            long afterSeed = Double.doubleToLongBits(random.nextGaussian());
            out.println("G\t" + algorithm + "\t" + seed + "\t" + a + "\t" + between + "\t" + b + "\t" + integer + "\t" + c + "\t" + afterSeed + "\t" + random.nextLong());
        }
        var holders = registry.listElements().toList();
        var tags = new HashMap<String, List<net.minecraft.tags.TagKey<Enchantment>>>();
        var exclusiveTags = new ArrayList<>(holders.stream().map(holder -> holder.value().exclusiveSet().unwrapKey())
            .flatMap(Optional::stream).distinct().toList());
        exclusiveTags.add(net.minecraft.tags.EnchantmentTags.ON_MOB_SPAWN_EQUIPMENT);
        for (var tag : exclusiveTags) {
            for (var entry : tagEntries("enchantment", tag.location(), new HashSet<>()))
                tags.computeIfAbsent(entry, ignored -> new ArrayList<>()).add(tag);
        }
        var bindTags = net.minecraft.core.Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        bindTags.setAccessible(true);
        for (var holder : holders) bindTags.invoke(holder, tags.getOrDefault(id(holder), List.of()));
        var itemTags = holders.stream().map(holder -> holder.value().definition().primaryItems()
                .orElse(holder.value().definition().supportedItems()).unwrapKey())
            .flatMap(Optional::stream).distinct().toList();
        for (var item : List.of(Items.TRIDENT, Items.FISHING_ROD)) {
            String itemId = BuiltInRegistries.ITEM.getKey(item).toString();
            var membership = new ArrayList<net.minecraft.tags.TagKey<net.minecraft.world.item.Item>>();
            for (var tag : itemTags) if (tagEntries("item", tag.location(), new HashSet<>()).contains(itemId)) membership.add(tag);
            bindTags.invoke(item.builtInRegistryHolder(), membership);
        }
        var holdersById = new HashMap<String, Holder<Enchantment>>();
        for (var holder : holders) holdersById.put(id(holder), holder);
        var mobOrder = tagEntries("enchantment", net.minecraft.tags.EnchantmentTags.ON_MOB_SPAWN_EQUIPMENT.location(), new HashSet<>());
        for (int algorithm = 0; algorithm < 2; algorithm++) for (int i = 0; i < 512; i++) {
            long seed = i * 3456789L - 177L;
            float special = new float[]{0f, 0.1f, 0.25f, 0.5f, 0.75f, 1f}[i % 6];
            RandomSource random = algorithm == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
            int cost = net.minecraft.util.Mth.randomBetweenInclusive(random, 5, 5 + (int)(special * 17));
            out.println("D\t" + algorithm + "\t" + seed + "\t" + Float.floatToIntBits(special) + "\t" + cost + "\t" + random.nextLong());
        }
        for (int mode = 0; mode < 5; mode++) {
            var stack = new ItemStack(mode == 0 ? Items.TRIDENT : mode == 1 ? Items.FISHING_ROD : Items.BOOK);
            if (mode >= 2) stack.set(DataComponents.ENCHANTABLE, new Enchantable(mode == 2 ? 1 : 20));
            String itemId = mode == 0 ? "minecraft:trident" : mode == 1 ? "minecraft:fishing_rod" : "book_" + stack.get(DataComponents.ENCHANTABLE).value() + (mode == 4 ? "_duplicates" : "");
            int enchantability = stack.get(DataComponents.ENCHANTABLE).value();
            final boolean weapon = mode < 2;
            List<Holder<Enchantment>> candidates = weapon
                ? mobOrder.stream().map(holdersById::get).filter(holder -> holder.value().isPrimaryItem(stack)).toList()
                : holders.stream().map(holder -> (Holder<Enchantment>) holder).toList();
            if (mode == 4) {
                candidates = new ArrayList<>(candidates);
                candidates.add(2, candidates.get(0));
            }
            for (var holder : candidates) {
                var enchantment = holder.value();
                var costs = new ArrayList<String>();
                for (int level = enchantment.getMinLevel(); level <= enchantment.getMaxLevel(); level++) {
                    costs.add(level + ":" + enchantment.getMinCost(level) + ":" + enchantment.getMaxCost(level));
                }
                String exclusive = candidates.stream().filter(other -> other != holder && !Enchantment.areCompatible(holder, other))
                    .map(EnchantmentSelectionOracle::id).collect(Collectors.joining(","));
                out.println("C\t" + itemId + "\t" + id(holder) + "\t" + enchantment.getWeight() + "\t" + String.join(",", costs) + "\t" + exclusive);
            }
            for (int algorithm = 0; algorithm < 2; algorithm++) for (int i = 0; i < 512; i++) {
                long seed = i * 1234567L - 991L;
                int cost = new int[]{Integer.MIN_VALUE, -20, 1, 5, 10, 20, 30, 60, 100, Integer.MAX_VALUE}[i % 10];
                RandomSource random = algorithm == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                var result = EnchantmentHelper.selectEnchantment(random, stack, cost, candidates.stream());
                String selected = result.stream().map(value -> id(value.enchantment()) + "=" + value.level()).collect(Collectors.joining(","));
                out.println("R\t" + itemId + "\t" + algorithm + "\t" + seed + "\t" + cost + "\t" + enchantability + "\t" + selected + "\t" + random.nextLong());
            }
            if (weapon) for (int algorithm = 0; algorithm < 2; algorithm++) for (int i = 0; i < 512; i++) {
                long seed = i * 7654321L - 321L;
                float special = new float[]{0f, 0.1f, 0.25f, 0.5f, 0.75f, 1f}[i % 6];
                RandomSource random = algorithm == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                int cost = net.minecraft.util.Mth.randomBetweenInclusive(random, 5, 5 + (int)(special * 17));
                var result = EnchantmentHelper.selectEnchantment(random, stack, cost, candidates.stream());
                String selected = result.stream().map(value -> id(value.enchantment()) + "=" + value.level()).collect(Collectors.joining(","));
                var enchanted = stack.copy();
                for (var value : result) enchanted.enchant(value.enchantment(), value.level());
                var encoded = ItemStack.CODEC.encodeStart(lookup.createSerializationContext(com.mojang.serialization.JsonOps.INSTANCE), enchanted).getOrThrow();
                out.println("P\t" + itemId + "\t" + algorithm + "\t" + seed + "\t" + Float.floatToIntBits(special) + "\t" + enchantability + "\t" + selected + "\t" + random.nextLong() + "\t" + encoded);
            }
            if (weapon) for (int algorithm = 0; algorithm < 2; algorithm++) for (int i = 0; i < 1152; i++) {
                long seed = i * 8765431L - 123L;
                int variant = i % 9;
                IntProvider number = switch (variant) {
                    case 0 -> ConstantInt.of(-2);
                    case 1 -> UniformInt.of(0, 8);
                    case 2 -> BiasedToBottomInt.of(-3, 12);
                    case 3 -> TrapezoidInt.of(0, 12, 2);
                    case 4 -> ClampedInt.of(UniformInt.of(-10, 20), 1, 7);
                    case 5 -> ConstantInt.of(Integer.MAX_VALUE);
                    case 6 -> new WeightedListInt(net.minecraft.util.random.WeightedList.<IntProvider>builder().add(ConstantInt.of(2), 3).add(UniformInt.of(3, 9), 2).build());
                    case 7 -> VeryBiasedToBottomInt.of(-5, 13);
                    default -> ClampedNormalInt.of(3.25f, 4.75f, -10, 15);
                };
                RandomSource random = algorithm == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                var mutable = new ItemEnchantments.Mutable(ItemEnchantments.EMPTY);
                if ((i / 9) % 2 == 0) new SingleEnchantment(holdersById.get("minecraft:loyalty"), number).enchant(stack, mutable, random, null);
                else new EnchantmentsByCost(net.minecraft.core.HolderSet.direct(candidates), number).enchant(stack, mutable, random, null);
                String selected = mutable.keySet().stream().map(holder -> id(holder) + "=" + mutable.getLevel(holder)).sorted().collect(Collectors.joining(","));
                out.println("Q\t" + itemId + "\t" + algorithm + "\t" + seed + "\t" + ((i / 9) % 2) + "\t" + variant + "\t" + enchantability + "\t" + selected + "\t" + random.nextLong());
                if (variant == 8) {
                    RandomSource continued = algorithm == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                    var provider = (i / 9) % 2 == 0 ? (net.minecraft.world.item.enchantment.providers.EnchantmentProvider) new SingleEnchantment(holdersById.get("minecraft:loyalty"), number) : new EnchantmentsByCost(net.minecraft.core.HolderSet.direct(candidates), number);
                    var first = new ItemEnchantments.Mutable(ItemEnchantments.EMPTY);
                    var second = new ItemEnchantments.Mutable(ItemEnchantments.EMPTY);
                    provider.enchant(stack, first, continued, null);
                    int intermediate = continued.nextInt();
                    provider.enchant(stack, second, continued, null);
                    String a = first.keySet().stream().map(holder -> id(holder) + "=" + first.getLevel(holder)).sorted().collect(Collectors.joining(","));
                    String b = second.keySet().stream().map(holder -> id(holder) + "=" + second.getLevel(holder)).sorted().collect(Collectors.joining(","));
                    out.println("H\t" + itemId + "\t" + algorithm + "\t" + seed + "\t" + ((i / 9) % 2) + "\t" + variant + "\t" + a + "\t" + intermediate + "\t" + b + "\t" + continued.nextLong());
                }
            }
        }
    }
}
