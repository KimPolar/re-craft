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
import net.minecraft.tags.BiomeTags;
import net.minecraft.util.ProblemReporter;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.animal.sheep.Sheep;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.storage.TagValueOutput;
import sun.misc.Unsafe;

public class GeneratedSheepOracle {
    static class World extends ServerLevel {
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public int getNextEntityId() { return 1; }
    }
    static class TestSheep extends Sheep {
        TestSheep(World world) { super(EntityTypes.SHEEP, world); }
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level) { return null; }
        @Override protected void registerGoals() {}
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world=(World)((Unsafe)field.get(null)).allocateInstance(World.class);
        var lookup=VanillaRegistries.createWorldLookup();
        var input=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        for (var entry : input) {
            var test=entry.getAsJsonObject();
            long seed=test.get("seed").getAsLong();
            RandomSource root=test.get("legacy").getAsBoolean() ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
            var factory=root.forkPositional().fromHashOf(Identifier.withDefaultNamespace("worldgen_region_random")).forkPositional();
            var random=factory.at(test.get("x").getAsInt()<<4,0,test.get("z").getAsInt()<<4);
            var biome=Holder.Reference.createStandAlone(new HolderOwner<Biome>(){}, Biomes.PLAINS);
            var tags=new ArrayList<net.minecraft.tags.TagKey<Biome>>();
            if(test.get("warm").getAsBoolean()) tags.add(BiomeTags.SPAWNS_WARM_VARIANT_FARM_ANIMALS);
            if(test.get("cold").getAsBoolean()) tags.add(BiomeTags.SPAWNS_COLD_VARIANT_FARM_ANIMALS);
            var bindTags=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bindTags.setAccessible(true); bindTags.invoke(biome,tags);
            var region=(ServerLevelAccessor)java.lang.reflect.Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)-> {
                if(m.getName().equals("getRandom")) return random;
                if(m.getName().equals("getBiome")) return biome;
                throw new AssertionError(m);
            });
            var group=new AgeableMob.AgeableMobGroupData(true,test.get("chance").getAsFloat());
            int index=0;
            for(var path : test.getAsJsonArray("paths")) {
                var sheep=new TestSheep(world); sheep.setUUID(new UUID(1,2));
                sheep.setPos(3.5,10,-2.5); sheep.setYRot(22.5f); sheep.setXRot(0);
                var result=sheep.finalizeSpawn(region,null,EntitySpawnReason.CHUNK_GENERATION,group);
                if(result!=group) throw new AssertionError("group identity");
                var output=TagValueOutput.createWithContext(ProblemReporter.DISCARDING,lookup);
                if(!sheep.save(output)) throw new AssertionError("sheep not saved");
                var javaTag=output.buildResult(); var rustTag=NbtIo.read(Path.of(path.getAsString()));
                if(!javaTag.equals(rustTag)) throw new AssertionError("case="+test+" index="+index+" java="+javaTag+" rust="+rustTag);
                index++;
            }
        }
    }
}
