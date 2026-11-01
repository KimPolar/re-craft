import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.world.attribute.*;
import net.minecraft.world.clock.*;
import net.minecraft.world.timeline.AttributeTrack;

public class SkyLightOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var item : input) {
            var c = item.getAsJsonObject();
            var attribute = EnvironmentAttributes.SKY_LIGHT_LEVEL;
            var map = EnvironmentAttributeMap.CODEC.parse(JsonOps.INSTANCE, c.get("attributes")).getOrThrow();
            float value = attribute.defaultValue();
            var entry = map.get(attribute);
            if (entry != null) value = entry.applyModifier(value);
            if (c.has("track")) {
                var track = AttributeTrack.createCodec(attribute).parse(JsonOps.INSTANCE, c.get("track")).getOrThrow();
                var clock = new ServerClockManager.ServerClockInstance();
                ClockManager manager = ignored -> clock;
                clock.loadFrom(new ClockState(c.get("tick").getAsLong(), 0, 1, false));
                var sampler = track.bakeSampler(attribute, Holder.direct(new WorldClock()), Optional.of(24000), manager);
                value = sampler.applyTimeBased(value, 1);
            }
            float rain = c.get("rain").getAsFloat();
            float thunder = c.get("thunder").getAsFloat() * rain;
            var interpolation = attribute.type().stateChangeLerp();
            if (rain - thunder > 0) value = interpolation.apply(rain - thunder, value, WeatherAttributes.RAIN.get(attribute).applyModifier(value));
            if (thunder > 0) value = interpolation.apply(thunder, value, WeatherAttributes.THUNDER.get(attribute).applyModifier(value));
            value = attribute.sanitizeValue(value);
            var row = new JsonArray(); row.add(value); row.add((int)(15f - value)); output.add(row);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
