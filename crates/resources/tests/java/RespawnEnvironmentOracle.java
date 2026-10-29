import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.attribute.BedRule;
import net.minecraft.world.attribute.EnvironmentAttributeMap;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.attribute.EnvironmentAttributes;

/** Exercise the installed 26.3 codecs and layer samplers. */
public class RespawnEnvironmentOracle {
    static JsonObject bed(BedRule value) {
        JsonObject result = new JsonObject();
        result.addProperty("can_sleep", value.canSleep().getSerializedName());
        result.addProperty("can_set_spawn", value.canSetSpawn().getSerializedName());
        result.addProperty("destroy_on_use", value.destroyOnUse());
        result.addProperty("destroy_on_leave", value.destroyOnLeave());
        return result;
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray output = new JsonArray();
        for (JsonElement item : cases) {
            var builder = EnvironmentAttributeSystem.builder();
            for (JsonElement layer : item.getAsJsonArray()) {
                var map = EnvironmentAttributeMap.CODEC.parse(JsonOps.INSTANCE, layer).getOrThrow();
                builder.addConstantLayer(map);
            }
            var system = builder.build();
            JsonObject row = new JsonObject();
            row.add("bed", bed(system.getDimensionValue(EnvironmentAttributes.BED_RULE)));
            row.add("straw_bed", bed(system.getDimensionValue(EnvironmentAttributes.STRAW_BED_RULE)));
            row.addProperty("anchor", system.getDimensionValue(EnvironmentAttributes.RESPAWN_ANCHOR_WORKS));
            row.addProperty("fast_lava", system.getDimensionValue(EnvironmentAttributes.FAST_LAVA));
            output.add(row);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
    }
}
