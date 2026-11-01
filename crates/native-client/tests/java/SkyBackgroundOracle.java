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
import net.minecraft.util.ARGB;
import net.minecraft.util.Mth;
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
            row.add(interpolator.applyAttributeLayer(EnvironmentAttributes.SUN_ANGLE, 20f));
            var sunset = interpolator.applyAttributeLayer(EnvironmentAttributes.SUNRISE_SUNSET_COLOR, new org.joml.Vector4f(0.2f,0.4f,0.6f,0.8f));
            row.add(sunset.x()); row.add(sunset.y()); row.add(sunset.z()); row.add(sunset.w());
        }
        for (String operation : List.of("override", "add", "subtract", "multiply", "minimum", "maximum")) {
            var object = JsonParser.parseString("{\"minecraft:visual/fog_start_distance\":{\"modifier\":\"" + operation + "\",\"argument\":3.0}}");
            var map = EnvironmentAttributeMap.CODEC.parse(JsonOps.INSTANCE, object).getOrThrow();
            var interpolator = new SpatialAttributeInterpolator();
            interpolator.accumulate(1.0, map);
            var row = new JsonArray(); row.add(interpolator.applyAttributeLayer(EnvironmentAttributes.FOG_START_DISTANCE, 10f)); output.add(row);
        }
        for (var item : input.getAsJsonArray("fogColorCases")) {
            var c = item.getAsJsonObject();
            float distance = Math.min(c.get("end").getAsFloat() / 16f, c.get("chunks").getAsInt());
            float t = 1f - (float)Math.pow(Mth.clampedLerp(distance / 32f, 0.25f, 1f), 0.25);
            var color = ARGB.srgbLerp(t, new Vector3f(0.2f, 0.6f, 0.9f), new Vector3f(0.8f, 0.3f, 0.05f));
            var row = new JsonArray(); row.add(color.x()); row.add(color.y()); row.add(color.z()); output.add(row);
        }
        var weatherDarken = net.minecraft.client.renderer.fog.environment.AtmosphericFogEnvironment.class
            .getDeclaredMethod("applyWeatherDarken", org.joml.Vector3fc.class, float.class, float.class);
        weatherDarken.setAccessible(true);
        for (var item : input.getAsJsonArray("weatherCases")) {
            var c = item.getAsJsonObject();
            var result = (org.joml.Vector3fc) weatherDarken.invoke(null, new Vector3f(0.8f, 0.3f, 0.05f), c.get("rain").getAsFloat(), c.get("thunder").getAsFloat());
            var row = new JsonArray(); row.add(result.x()); row.add(result.y()); row.add(result.z()); output.add(row);
        }
        for (var item : input.getAsJsonArray("sunriseCases")) {
            var c = item.getAsJsonObject();
            var fog = new Vector3f(0.2f, 0.6f, 0.9f);
            var color = new Vector3f(0.9f, 0.4f, 0.1f);
            float direction = c.get("forward").getAsFloat() * (Mth.sin(c.get("angle").getAsFloat() * 0.017453292f) > 0f ? -1f : 1f);
            var result = c.get("chunks").getAsInt() >= 4 && direction > 0f && c.get("alpha").getAsFloat() > 0f
                ? ARGB.srgbLerp(direction * c.get("alpha").getAsFloat(), fog, color) : fog;
            var row = new JsonArray(); row.add(result.x()); row.add(result.y()); row.add(result.z()); output.add(row);
        }
        for (var item : input.getAsJsonArray("sunsetModifierCases")) {
            var c = item.getAsJsonObject();
            var map = EnvironmentAttributeMap.CODEC.parse(JsonOps.INSTANCE, c.get("attributes")).getOrThrow();
            var spatial = new SpatialAttributeInterpolator(); spatial.accumulate(1.0, map);
            var color = spatial.applyAttributeLayer(EnvironmentAttributes.SUNRISE_SUNSET_COLOR, new org.joml.Vector4f(0.2f,0.4f,0.6f,0.8f));
            var row = new JsonArray(); row.add(color.x()); row.add(color.y()); row.add(color.z()); row.add(color.w()); output.add(row);
        }
        for (var item : input.getAsJsonArray("sunsetTrackCases")) {
            var c = item.getAsJsonObject();
            var sunsetTrack = AttributeTrack.createCodec(EnvironmentAttributes.SUNRISE_SUNSET_COLOR).parse(JsonOps.INSTANCE, c.get("track")).getOrThrow();
            var sunsetSampler = sunsetTrack.bakeSampler(EnvironmentAttributes.SUNRISE_SUNSET_COLOR, Holder.direct(new WorldClock()), Optional.of(20), manager);
            for (var tick : c.getAsJsonArray("ticks")) {
                clock.loadFrom(new ClockState(tick.getAsLong(),0,1,false));
                var color = sunsetSampler.applyTimeBased(new org.joml.Vector4f(0.2f,0.4f,0.6f,0.8f), ++tickId);
                var row = new JsonArray(); row.add(color.x()); row.add(color.y()); row.add(color.z()); row.add(color.w()); output.add(row);
            }
        }
        var moonTrack = AttributeTrack.createCodec(EnvironmentAttributes.MOON_ANGLE)
            .parse(JsonOps.INSTANCE, input.get("moonTrack")).getOrThrow();
        var moonSampler = moonTrack.bakeSampler(EnvironmentAttributes.MOON_ANGLE, Holder.direct(new WorldClock()), Optional.of(24000), manager);
        for (int tick = -100; tick <= 24100; tick += 37) {
            clock.loadFrom(new ClockState(tick, 0, 1, false));
            var row = new JsonArray(); row.add(moonSampler.applyTimeBased(0f, ++tickId)); output.add(row);
        }
        for (var item : List.of(
                Map.entry(EnvironmentAttributes.STAR_ANGLE, "starAngleTrack"),
                Map.entry(EnvironmentAttributes.STAR_BRIGHTNESS, "starBrightnessTrack"))) {
            var starTrack = AttributeTrack.createCodec(item.getKey()).parse(JsonOps.INSTANCE, input.get(item.getValue())).getOrThrow();
            var starSampler = starTrack.bakeSampler(item.getKey(), Holder.direct(new WorldClock()), Optional.of(24000), manager);
            for (int tick = -100; tick <= 24100; tick += 37) {
                clock.loadFrom(new ClockState(tick, 0, 1, false));
                var row = new JsonArray(); row.add(starSampler.applyTimeBased(0f, ++tickId)); output.add(row);
            }
        }
        var starWeatherRain = WeatherAttributes.RAIN.get(EnvironmentAttributes.STAR_BRIGHTNESS);
        var starWeatherThunder = WeatherAttributes.THUNDER.get(EnvironmentAttributes.STAR_BRIGHTNESS);
        var starWeatherLerp = EnvironmentAttributes.STAR_BRIGHTNESS.type().stateChangeLerp();
        for (var item : input.getAsJsonArray("weatherCases")) {
            var c = item.getAsJsonObject();
            float rain = c.get("rain").getAsFloat(), thunder = c.get("thunder").getAsFloat();
            float value = 0.5f, rainOnly = rain - thunder;
            if (rainOnly > 0f) value = starWeatherLerp.apply(rainOnly, value, starWeatherRain.applyModifier(value));
            if (thunder > 0f) value = starWeatherLerp.apply(thunder, value, starWeatherThunder.applyModifier(value));
            var row = new JsonArray(); row.add(value); output.add(row);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
    }
}
