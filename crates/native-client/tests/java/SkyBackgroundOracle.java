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
import net.minecraft.world.phys.Vec3;
import org.joml.Vector3f;

public class SkyBackgroundOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
        var maps = new ArrayList<EnvironmentAttributeMap>();
        for (var layer : input.getAsJsonArray("biomes"))
            maps.add(EnvironmentAttributeMap.CODEC.parse(JsonOps.INSTANCE, layer).getOrThrow());
        var track = AttributeTrack.createCodec(EnvironmentAttributes.SKY_COLOR)
            .parse(JsonOps.INSTANCE, input.get("track")).getOrThrow();
        var clock = new ServerClockManager.ServerClockInstance(); ClockManager manager = ignored -> clock;
        var sampler = track.bakeSampler(EnvironmentAttributes.SKY_COLOR, Holder.direct(new WorldClock()), Optional.of(24000), manager);
        var fogTrack = AttributeTrack.createCodec(EnvironmentAttributes.FOG_END_DISTANCE)
            .parse(JsonOps.INSTANCE, input.get("fogTrack")).getOrThrow();
        var fogSampler = fogTrack.bakeSampler(EnvironmentAttributes.FOG_END_DISTANCE, Holder.direct(new WorldClock()), Optional.of(24000), manager);
        var output = new JsonArray(); int tickId = 0;
        for (var item : input.getAsJsonArray("cases")) {
            var c = item.getAsJsonObject(); var p = c.getAsJsonArray("position");
            var interpolator = new SpatialAttributeInterpolator();
            GaussianSampler.sample(new Vec3(p.get(0).getAsDouble(), p.get(1).getAsDouble(), p.get(2).getAsDouble()).scale(0.25),
                (x,y,z) -> maps.get(Math.floorMod(x + 3*y + 7*z, 3)),
                (weight, map) -> interpolator.accumulate(weight, map));
            var biome = interpolator.applyAttributeLayer(EnvironmentAttributes.SKY_COLOR, new Vector3f(120f/255,167f/255,1));
            clock.loadFrom(new ClockState(c.get("tick").getAsLong(),0,1,false));
            var result = sampler.applyTimeBased(biome, ++tickId);
            var row = new JsonArray(); row.add(result.x()); row.add(result.y()); row.add(result.z()); output.add(row);
            var fog = interpolator.applyAttributeLayer(EnvironmentAttributes.FOG_COLOR, new Vector3f());
            row.add(fog.x()); row.add(fog.y()); row.add(fog.z());
            var fogStart = interpolator.applyAttributeLayer(EnvironmentAttributes.FOG_START_DISTANCE, 10f);
            var fogEnd = interpolator.applyAttributeLayer(EnvironmentAttributes.FOG_END_DISTANCE, 96f);
            row.add(fogStart); row.add(fogSampler.applyTimeBased(fogEnd, tickId));
        }
        for (String operation : List.of("override", "add", "subtract", "multiply", "minimum", "maximum")) {
            var object = JsonParser.parseString("{\"minecraft:visual/fog_start_distance\":{\"modifier\":\"" + operation + "\",\"argument\":3.0}}");
            var map = EnvironmentAttributeMap.CODEC.parse(JsonOps.INSTANCE, object).getOrThrow();
            var interpolator = new SpatialAttributeInterpolator();
            interpolator.accumulate(1.0, map);
            var row = new JsonArray(); row.add(interpolator.applyAttributeLayer(EnvironmentAttributes.FOG_START_DISTANCE, 10f)); output.add(row);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
    }
}
