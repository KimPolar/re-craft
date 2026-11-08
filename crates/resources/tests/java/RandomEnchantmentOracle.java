import com.google.gson.JsonParser;
import com.google.gson.JsonArray;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.Constructor;
import java.util.List;
import java.util.Map;
import java.util.LinkedHashSet;
import java.util.HashSet;
import java.util.Set;
import java.util.ArrayList;
import java.util.Optional;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.RegistryOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.storage.loot.LootContext;
import net.minecraft.world.level.storage.loot.LootParams;
import net.minecraft.world.level.storage.loot.functions.EnchantRandomlyFunction;

public class RandomEnchantmentOracle {
    static Set<String> tagEntries(String kind, net.minecraft.resources.Identifier id, Set<String> visiting) throws Exception {
        if (!visiting.add(id.toString())) throw new IllegalStateException("cyclic tag");
        String path = "data/" + id.getNamespace() + "/tags/" + kind + "/" + id.getPath() + ".json";
        var result = new LinkedHashSet<String>();
        try (var stream = RandomEnchantmentOracle.class.getClassLoader().getResourceAsStream(path)) {
            if (stream == null) throw new IllegalStateException("missing tag " + path);
            var values = JsonParser.parseString(new String(stream.readAllBytes(), java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject().getAsJsonArray("values");
            for (var value : values) {
                String entry = value.isJsonObject() ? value.getAsJsonObject().get("id").getAsString() : value.getAsString();
                if (entry.startsWith("#")) result.addAll(tagEntries(kind, net.minecraft.resources.Identifier.parse(entry.substring(1)), visiting));
                else result.add(net.minecraft.resources.Identifier.parse(entry).toString());
            }
        }
        visiting.remove(id.toString());
        return result;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var holders = lookup.lookupOrThrow(Registries.ENCHANTMENT).listElements().toList();
        var itemTags = holders.stream().map(holder -> holder.value().definition().supportedItems().unwrapKey())
            .flatMap(Optional::stream).distinct().toList();
        var bindTags = net.minecraft.core.Holder.Reference.class.getDeclaredMethod("bindTags", java.util.Collection.class);
        bindTags.setAccessible(true);
        var swordTags = new ArrayList<net.minecraft.tags.TagKey<Item>>();
        for (var tag : itemTags) if (tagEntries("item", tag.location(), new HashSet<>()).contains("minecraft:diamond_sword")) swordTags.add(tag);
        bindTags.invoke(Items.DIAMOND_SWORD.builtInRegistryHolder(), swordTags);
        int total = 0, missing = 0;
        for (var item : BuiltInRegistries.ITEM) {
            var stack = new ItemStack(item);
            if (!stack.isEmpty()) {
                total++;
                if (!stack.has(DataComponents.ENCHANTMENTS)) missing++;
            }
        }
        System.out.printf("A %d %d%n", total, missing);
        var ops = RegistryOps.create(JsonOps.INSTANCE, registries);
        Constructor<?> ctor = LootContext.class.getDeclaredConstructors()[0];
        ctor.setAccessible(true);
        String[] options = {
            "{\"options\":\"#minecraft:on_random_loot\"}",
            "{\"options\":[\"minecraft:sharpness\",\"minecraft:protection\",\"minecraft:unbreaking\"]}",
            "{\"options\":[\"minecraft:unbreaking\"]}",
            "{\"options\":[\"minecraft:protection\"],\"only_compatible\":false}",
            "{\"options\":[]}",
        };
        Item[] items = {Items.BOOK, Items.DIAMOND_SWORD};
        for (int option = 0; option < options.length; option++) {
            var definition = JsonParser.parseString(options[option]).getAsJsonObject();
            if (option == 0) {
                var expanded = new JsonArray();
                for (String id : tagEntries("enchantment", net.minecraft.resources.Identifier.parse("minecraft:on_random_loot"), new HashSet<>())) expanded.add(id);
                definition.add("options", expanded);
            }
            var function = EnchantRandomlyFunction.MAP_CODEC.codec().parse(ops, definition).getOrThrow();
            for (int item = 0; item < items.length; item++) {
                for (int mode = 0; mode < 2; mode++) {
                    for (long seed : new long[]{0, 1, 42, -1, Long.MIN_VALUE, Long.MAX_VALUE}) {
                        RandomSource random = mode == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                        var params = new LootParams(null, ContextMap.EMPTY, Map.of(), 0.0f);
                        var context = (LootContext) ctor.newInstance(params, random, registries);
                        var stack = function.run(new ItemStack(items[item]), context);
                        var enchantments = stack.get(item == 0 ? DataComponents.STORED_ENCHANTMENTS : DataComponents.ENCHANTMENTS);
                        String selected = "-";
                        int level = 0;
                        if (enchantments != null && !enchantments.isEmpty()) {
                            if (enchantments.size() != 1) throw new AssertionError("unexpected enchantment count");
                            var entry = enchantments.entrySet().iterator().next();
                            selected = entry.getKey().unwrapKey().orElseThrow().identifier().toString();
                            level = entry.getIntValue();
                        }
                        System.out.printf("E %d %d %d %d %s %d %d%n", option, item, mode, seed, selected, level, random.nextLong());
                    }
                }
            }
        }
    }
}
