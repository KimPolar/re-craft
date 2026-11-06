import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.world.attribute.BedRule;
import net.minecraft.world.attribute.EnvironmentAttributes;
import net.minecraft.world.clock.*;
import net.minecraft.world.timeline.AttributeTrack;

public class RespawnTimelineOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var entry : cases) {
            var test = entry.getAsJsonObject();
            var period = test.has("period") ? Optional.of(test.get("period").getAsInt()) : Optional.<Integer>empty();
            var holder = Holder.direct(new WorldClock());
            var clock = new ServerClockManager.ServerClockInstance();
            ClockManager manager = ignored -> clock;
            var rows = new JsonArray();
            int tickId = 0;
            if (test.get("kind").getAsString().equals("anchor")) {
                var track = AttributeTrack.createCodec(EnvironmentAttributes.RESPAWN_ANCHOR_WORKS)
                    .parse(JsonOps.INSTANCE, test.get("track")).getOrThrow();
                var sampler = track.bakeSampler(EnvironmentAttributes.RESPAWN_ANCHOR_WORKS, holder, period, manager);
                for (var value : test.getAsJsonArray("ticks")) {
                    clock.loadFrom(new ClockState(value.getAsLong(), 0.0f, 1.0f, false));
                    rows.add(sampler.applyTimeBased(test.get("base").getAsBoolean(), ++tickId));
                }
            } else {
                var track = AttributeTrack.createCodec(EnvironmentAttributes.BED_RULE)
                    .parse(JsonOps.INSTANCE, test.get("track")).getOrThrow();
                var sampler = track.bakeSampler(EnvironmentAttributes.BED_RULE, holder, period, manager);
                var base = new BedRule(BedRule.Rule.WHEN_DARK, BedRule.Rule.ALWAYS, false, false, Optional.empty());
                for (var value : test.getAsJsonArray("ticks")) {
                    clock.loadFrom(new ClockState(value.getAsLong(), 0.0f, 1.0f, false));
                    var bed = sampler.applyTimeBased(base, ++tickId);
                    var row = new JsonObject();
                    row.addProperty("can_sleep", bed.canSleep().getSerializedName());
                    row.addProperty("can_set_spawn", bed.canSetSpawn().getSerializedName());
                    row.addProperty("destroy_on_use", bed.destroyOnUse());
                    row.addProperty("destroy_on_leave", bed.destroyOnLeave());
                    rows.add(row);
                }
            }
            output.add(rows);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
    }
}
