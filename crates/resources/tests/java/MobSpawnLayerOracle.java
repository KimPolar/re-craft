import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.attribute.EnvironmentAttributeMap;
import net.minecraft.world.attribute.EnvironmentAttributes;
import net.minecraft.world.entity.MobCategory;
import net.minecraft.world.level.levelgen.LegacyRandomSource;

public final class MobSpawnLayerOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var item : cases) {
            var test = item.getAsJsonObject();
            var attribute = EnvironmentAttributes.NATURAL_MOB_SPAWNS;
            var settings = attribute.defaultValue();
            for (var layer : test.getAsJsonArray("layers")) {
                var map = EnvironmentAttributeMap.CODEC.parse(JsonOps.INSTANCE, layer).getOrThrow();
                settings = map.applyModifier(attribute, settings);
            }
            var spawns = settings.getMobsToSpawn(MobCategory.CREATURE);
            var random = new LegacyRandomSource(test.get("seed").getAsLong());
            var samples = new JsonArray();
            for (int i = 0; i < 24; i++) {
                var choice = spawns.getRandom(random);
                if (choice.isEmpty()) samples.add(JsonNull.INSTANCE);
                else {
                    var entry = choice.orElseThrow();
                    var sample = new JsonArray();
                    sample.add(BuiltInRegistries.ENTITY_TYPE.getKey(entry.type()).toString());
                    sample.add(entry.count().sample(random));
                    samples.add(sample);
                }
            }
            var result = new JsonObject();
            result.add("samples", samples);
            result.addProperty("next", random.nextInt());
            output.add(result);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
