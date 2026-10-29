import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.world.attribute.EnvironmentAttributes;
import net.minecraft.world.clock.*;
import net.minecraft.world.timeline.AttributeTrack;

public class FastLavaTimelineOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var entry : cases) {
            var test = entry.getAsJsonObject();
            var track = AttributeTrack.createCodec(EnvironmentAttributes.FAST_LAVA)
                .parse(JsonOps.INSTANCE, test.get("track")).getOrThrow();
            var period = test.has("period") ? Optional.of(test.get("period").getAsInt()) : Optional.<Integer>empty();
            var holder = Holder.direct(new WorldClock());
            var clock = new ServerClockManager.ServerClockInstance();
            ClockManager manager = ignored -> clock;
            var sampler = track.bakeSampler(EnvironmentAttributes.FAST_LAVA, holder, period, manager);
            var rows = new JsonArray(); int tickId = 0;
            for (var value : test.getAsJsonArray("ticks")) {
                clock.loadFrom(new ClockState(value.getAsLong(), 0.0f, 1.0f, false));
                rows.add(sampler.applyTimeBased(test.get("base").getAsBoolean(), ++tickId));
            }
            output.add(rows);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
    }
}
