import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BiomeTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.variant.PriorityProvider;
import net.minecraft.world.entity.variant.SpawnContext;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

public class FarmAnimalVariantOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonObject();
        var tables = new JsonObject();
        var pig = lookup.lookupOrThrow(Registries.PIG_VARIANT).listElements().toList();
        var cow = lookup.lookupOrThrow(Registries.COW_VARIANT).listElements().toList();
        var chicken = lookup.lookupOrThrow(Registries.CHICKEN_VARIANT).listElements().toList();
        for (var item : java.util.List.of(java.util.Map.entry("pig",pig), java.util.Map.entry("cow",cow), java.util.Map.entry("chicken",chicken))) {
            var ids = new JsonArray();
            for (var holder : item.getValue()) ids.add(holder.key().identifier().toString());
            tables.add(item.getKey(),ids);
        }
        output.add("order",tables);
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
            result.addProperty("next",random.nextLong()); results.add(result);
        }
        output.add("results",results);
        Files.writeString(Path.of(args[1]),output.toString());
    }
}
