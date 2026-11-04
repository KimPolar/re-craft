import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.attribute.*;
import net.minecraft.world.clock.*;
import net.minecraft.world.entity.MobCategory;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.timeline.AttributeTrack;

public class CreatureTimelineOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output=new JsonArray(); int tickId=0;
        for (var item:cases) {
            var c=item.getAsJsonObject();
            var clock=new ServerClockManager.ServerClockInstance(); ClockManager manager=ignored->clock;
            Optional<Integer> period=c.has("period") && !c.get("period").isJsonNull() ? Optional.of(c.get("period").getAsInt()) : Optional.empty();
            var rows=new JsonArray();
            if(c.get("kind").getAsString().equals("probability")) {
                var attr=EnvironmentAttributes.CREATURE_WORLD_GEN_SPAWN_PROBABILITY;
                var track=AttributeTrack.createCodec(attr).parse(JsonOps.INSTANCE,c.get("track")).getOrThrow();
                var sampler=track.bakeSampler(attr,Holder.direct(new WorldClock()),period,manager);
                for(var tick:c.getAsJsonArray("ticks")) {
                    clock.loadFrom(new ClockState(tick.getAsLong(),0,1,false));
                    float value=attr.sanitizeValue(sampler.applyTimeBased(0.1f,++tickId));
                    rows.add(Float.floatToRawIntBits(value));
                }
            } else {
                var attr=EnvironmentAttributes.NATURAL_MOB_SPAWNS;
                var track=AttributeTrack.createCodec(attr).parse(JsonOps.INSTANCE,c.get("track")).getOrThrow();
                var sampler=track.bakeSampler(attr,Holder.direct(new WorldClock()),period,manager);
                for(var tick:c.getAsJsonArray("ticks")) {
                    clock.loadFrom(new ClockState(tick.getAsLong(),0,1,false));
                    var settings=sampler.applyTimeBased(attr.defaultValue(),++tickId);
                    var choice=settings.getMobsToSpawn(MobCategory.CREATURE).getRandom(new LegacyRandomSource(42));
                    rows.add(choice.map(entry->BuiltInRegistries.ENTITY_TYPE.getKey(entry.type()).toString()).orElse("empty"));
                }
            }
            output.add(rows);
        }
        Files.writeString(Path.of(args[1]),output.toString());
    }
}
