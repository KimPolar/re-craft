import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.*;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.monster.Witch;
import net.minecraft.world.level.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.storage.TagValueOutput;
import sun.misc.Unsafe;

public class GeneratedHutWitchOracle {
    static class World extends ServerLevel {
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public int getNextEntityId() { return 1; }
    }
    static class TestWitch extends Witch {
        TestWitch(World world) { super(EntityTypes.WITCH,world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return null; }
        @Override protected void registerGoals() {}
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world=(World)((Unsafe)field.get(null)).allocateInstance(World.class);
        var lookup=VanillaRegistries.createWorldLookup();
        var input=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        for(var entry:input) {
            var test=entry.getAsJsonObject();long seed=test.get("seed").getAsLong();
            RandomSource root=test.get("legacy").getAsBoolean()?new LegacyRandomSource(seed):new XoroshiroRandomSource(seed);
            var random=root.forkPositional().fromHashOf(Identifier.withDefaultNamespace("worldgen_region_random")).forkPositional().at(-13<<4,0,17<<4);
            var region=(ServerLevelAccessor)java.lang.reflect.Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)->{
                if(m.getName().equals("getRandom"))return random;throw new AssertionError(m);
            });
            var witch=new TestWitch(world);witch.setUUID(new UUID(1,2));witch.setPersistenceRequired();witch.snapTo(3.5,10,-2.5,0,0);
            witch.finalizeSpawn(region,null,EntitySpawnReason.STRUCTURE,null);
            var output=TagValueOutput.createWithContext(ProblemReporter.DISCARDING,lookup);
            if(!witch.save(output))throw new AssertionError("witch not saved");
            var actual=output.buildResult();var expected=NbtIo.read(Path.of(test.get("path").getAsString()));
            if(!actual.equals(expected))throw new AssertionError("case="+test+" java="+actual+" rust="+expected);
            if(random.nextLong()!=test.get("next").getAsLong())throw new AssertionError("random stream differs: "+test);
        }
    }
}
