import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.animal.turtle.Turtle;
import net.minecraft.world.level.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.storage.TagValueOutput;
import sun.misc.Unsafe;

public class GeneratedTurtleOracle {
    static class World extends ServerLevel {
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public int getNextEntityId() { return 1; }
    }
    static class TestTurtle extends Turtle {
        TestTurtle(World world) { super(EntityTypes.TURTLE, world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return null; }
        @Override protected void registerGoals() {}
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world=(World)((Unsafe)field.get(null)).allocateInstance(World.class);
        var lookup=VanillaRegistries.createWorldLookup();
        var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        for (var entry : cases) {
            var test=entry.getAsJsonObject();
            long seed=test.get("seed").getAsLong();
            RandomSource root=test.get("legacy").getAsBoolean() ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
            var random=root.forkPositional().fromHashOf(Identifier.withDefaultNamespace("worldgen_region_random")).forkPositional().at(-13<<4,0,17<<4);
            var region=(ServerLevelAccessor)java.lang.reflect.Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)-> {
                if(m.getName().equals("getRandom")) return random;
                throw new AssertionError(m);
            });
            var group=new AgeableMob.AgeableMobGroupData(true,test.get("chance").getAsFloat());
            var pos=test.getAsJsonArray("position");
            int index=0;
            for(var path : test.getAsJsonArray("paths")) {
                var turtle=new TestTurtle(world); turtle.setUUID(new UUID(1,2));
                turtle.setPos(pos.get(0).getAsDouble(),pos.get(1).getAsDouble(),pos.get(2).getAsDouble());
                turtle.setYRot(22.5f); turtle.setXRot(0);
                if(turtle.finalizeSpawn(region,null,EntitySpawnReason.CHUNK_GENERATION,group)!=group)
                    throw new AssertionError("group identity changed");
                var output=TagValueOutput.createWithContext(ProblemReporter.DISCARDING,lookup);
                if(!turtle.save(output)) throw new AssertionError("turtle not saved");
                var javaTag=output.buildResult(); var rustTag=NbtIo.read(Path.of(path.getAsString()));
                if(!javaTag.equals(rustTag)) throw new AssertionError("case="+test+" index="+index+" java="+javaTag+" rust="+rustTag);
                index++;
            }
            if(random.nextLong()!=test.get("next").getAsLong()) throw new AssertionError("random mismatch: "+test);
        }
    }
}
