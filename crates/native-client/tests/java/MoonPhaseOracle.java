import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.world.attribute.EnvironmentAttributes;
import net.minecraft.world.clock.*;
import net.minecraft.world.level.MoonPhase;
import net.minecraft.world.timeline.AttributeTrack;

public class MoonPhaseOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
        var clock = new ServerClockManager.ServerClockInstance();
        ClockManager manager = ignored -> clock;
        var track = AttributeTrack.createCodec(EnvironmentAttributes.MOON_PHASE)
            .parse(JsonOps.INSTANCE, input.get("track")).getOrThrow();
        var sampler = track.bakeSampler(EnvironmentAttributes.MOON_PHASE,
            Holder.direct(new WorldClock()), Optional.of(192000), manager);
        var output = new JsonArray(); int update = 0;
        for (var tick : input.getAsJsonArray("ticks")) {
            clock.loadFrom(new ClockState(tick.getAsLong(), 0, 1, false));
            output.add(sampler.applyTimeBased(MoonPhase.FULL_MOON, ++update).index());
        }
        for (float partial : new float[] {0f, 0.25f, 0.999f, 1f})
            output.add(EnvironmentAttributes.MOON_PHASE.type().partialTickLerp()
                .apply(partial, MoonPhase.FULL_MOON, MoonPhase.NEW_MOON).index());
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
    }
}
