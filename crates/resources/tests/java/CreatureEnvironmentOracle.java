import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.attribute.*;
import net.minecraft.world.entity.MobCategory;
import net.minecraft.world.level.levelgen.LegacyRandomSource;

public class CreatureEnvironmentOracle {
    static EnvironmentAttributeMap map(JsonElement value) {
        return EnvironmentAttributeMap.CODEC.parse(JsonOps.INSTANCE,value).getOrThrow();
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output=new JsonArray();
        for (var value : cases) {
            var c=value.getAsJsonObject();
            var spawn=EnvironmentAttributes.NATURAL_MOB_SPAWNS;
            var probability=EnvironmentAttributes.CREATURE_WORLD_GEN_SPAWN_PROBABILITY;
            var dimension=map(c.get("dimension"));
            var settings=dimension.applyModifier(spawn,spawn.defaultValue());
            float p=dimension.applyModifier(probability,probability.defaultValue());
            settings=map(c.get("spawn_biome")).applyModifier(spawn,settings);
            p=map(c.get("probability_biome")).applyModifier(probability,p);
            for(var later:c.getAsJsonArray("later")) {
                var layer=map(later);
                settings=layer.applyModifier(spawn,settings);
                p=layer.applyModifier(probability,p);
            }
            p=probability.sanitizeValue(p);
            var list=settings.getMobsToSpawn(MobCategory.CREATURE);
            var random=new LegacyRandomSource(42);
            var samples=new JsonArray();
            for(int i=0;i<8;i++) {
                var selected=list.getRandom(random);
                if(selected.isEmpty()) samples.add(JsonNull.INSTANCE);
                else {
                    var entry=selected.orElseThrow(); var row=new JsonArray();
                    row.add(BuiltInRegistries.ENTITY_TYPE.getKey(entry.type()).toString());
                    row.add(entry.count().sample(random)); samples.add(row);
                }
            }
            var result=new JsonObject(); result.addProperty("bits",Float.floatToRawIntBits(p));
            result.add("samples",samples); result.addProperty("next",random.nextInt()); output.add(result);
        }
        Files.writeString(Path.of(args[1]),output.toString());
    }
}
