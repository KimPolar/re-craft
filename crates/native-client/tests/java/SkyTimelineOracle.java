import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.world.attribute.EnvironmentAttributes;
import net.minecraft.world.clock.*;
import net.minecraft.world.timeline.AttributeTrack;
import org.joml.Vector3f;

public class SkyTimelineOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
        var factor = AttributeTrack.createCodec(EnvironmentAttributes.SKY_LIGHT_FACTOR)
            .parse(JsonOps.INSTANCE, input.get("factor")).getOrThrow();
        var color = AttributeTrack.createCodec(EnvironmentAttributes.SKY_LIGHT_COLOR)
            .parse(JsonOps.INSTANCE, input.get("color")).getOrThrow();
        var sun = AttributeTrack.createCodec(EnvironmentAttributes.SUN_ANGLE)
            .parse(JsonOps.INSTANCE, input.get("sun")).getOrThrow();
        var sunset = AttributeTrack.createCodec(EnvironmentAttributes.SUNRISE_SUNSET_COLOR)
            .parse(JsonOps.INSTANCE, input.get("sunset")).getOrThrow();
        var clock = new ServerClockManager.ServerClockInstance();
        ClockManager manager = ignored -> clock;
        var holder = Holder.direct(new WorldClock());
        var factorSampler = factor.bakeSampler(EnvironmentAttributes.SKY_LIGHT_FACTOR, holder, Optional.of(24000), manager);
        var colorSampler = color.bakeSampler(EnvironmentAttributes.SKY_LIGHT_COLOR, holder, Optional.of(24000), manager);
        var sunSampler = sun.bakeSampler(EnvironmentAttributes.SUN_ANGLE, holder, Optional.of(24000), manager);
        var sunsetSampler = sunset.bakeSampler(EnvironmentAttributes.SUNRISE_SUNSET_COLOR, holder, Optional.of(24000), manager);
        var output = new JsonArray(); int id = 0;
        for (var tick : input.getAsJsonArray("ticks")) {
            clock.loadFrom(new ClockState(tick.getAsLong(),0,1,false));
            var row = new JsonObject();
            row.addProperty("factor", factorSampler.applyTimeBased(1.0f, ++id));
            row.addProperty("sun", sunSampler.applyTimeBased(0.0f, id));
            var sunsetColor = sunsetSampler.applyTimeBased(new org.joml.Vector4f(), id);
            var sunsetChannels = new JsonArray(); sunsetChannels.add(sunsetColor.x()); sunsetChannels.add(sunsetColor.y()); sunsetChannels.add(sunsetColor.z()); sunsetChannels.add(sunsetColor.w());
            row.add("sunset", sunsetChannels);
            var result = colorSampler.applyTimeBased(new Vector3f(1,1,1), id);
            var channels = new JsonArray(); channels.add(result.x()); channels.add(result.y()); channels.add(result.z());
            row.add("color", channels); output.add(row);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
    }
}
