import com.google.gson.JsonParser;
import java.lang.reflect.Proxy;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Comparator;
import java.util.List;
import java.util.UUID;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.network.syncher.EntityDataAccessor;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BiomeTags;
import net.minecraft.util.ProblemReporter;
import net.minecraft.util.RandomSource;
import net.minecraft.util.SpecialDates;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.ai.navigation.AmphibiousPathNavigation;
import net.minecraft.world.entity.ai.navigation.PathNavigation;
import net.minecraft.world.entity.monster.zombie.Drowned;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.biome.Biomes;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.storage.TagValueOutput;

public class GeneratedStructureDrownedOracle {
    static class RealDrowned extends Drowned {
        boolean doors;
        RealDrowned(StructureDrownedOracle.World world){super(EntityTypes.DROWNED,world);}
        @Override protected void registerGoals(){}
        @Override protected PathNavigation createNavigation(Level level){
            try{return (PathNavigation)StructureDrownedOracle.allocate.invoke(StructureDrownedOracle.unsafe,AmphibiousPathNavigation.class);}
            catch(ReflectiveOperationException error){throw new RuntimeException(error);}
        }
        @Override public void setCanBreakDoors(boolean value){doors=value;}
        @Override public boolean canBreakDoors(){return doors;}
        // Keep real setBaby and its transient movement modifier; dimensions
        // do not affect the NBT in these non-jockey cases.
        @Override public void onSyncedDataUpdated(EntityDataAccessor<?> key){}
    }
    static void normalize(CompoundTag root){
        var attributes=root.getList("attributes").orElseThrow();
        attributes.sort(Comparator.comparing(tag->((CompoundTag)tag).getStringOr("id","")));
        for(var tag:attributes) ((CompoundTag)tag).getList("modifiers").ifPresent(modifiers->
                modifiers.sort(Comparator.comparing(m->((CompoundTag)m).getStringOr("id",""))));
    }
    public static void main(String[] args) throws Exception {
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        if(args.length==1 && args[0].equals("--halloween")){out.println("H\t"+SpecialDates.isHalloween());return;}
        var lookup=VanillaRegistries.createWorldLookup();var registries=VanillaRegistries.createReloadableLookup(lookup);
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries))pending.apply();
        var uc=Class.forName("sun.misc.Unsafe");var uf=uc.getDeclaredField("theUnsafe");uf.setAccessible(true);
        StructureDrownedOracle.unsafe=uf.get(null);StructureDrownedOracle.allocate=uc.getMethod("allocateInstance",Class.class);
        var world=(StructureDrownedOracle.World)StructureDrownedOracle.allocate.invoke(StructureDrownedOracle.unsafe,StructureDrownedOracle.World.class);
        var entityRandom=Entity.class.getDeclaredField("random");entityRandom.setAccessible(true);
        var biome=registries.lookupOrThrow(Registries.BIOME).getOrThrow(Biomes.OCEAN);
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",java.util.Collection.class);bind.setAccessible(true);
        bind.invoke(biome,List.of(BiomeTags.MORE_FREQUENT_DROWNED_SPAWNS));
        var tests=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        for(var entry:tests){
            var test=entry.getAsJsonObject();long regionSeed=test.get("region_seed").getAsLong(),entitySeed=test.get("entity_seed").getAsLong();
            RandomSource random=test.get("legacy").getAsBoolean()?new LegacyRandomSource(regionSeed):new XoroshiroRandomSource(regionSeed);
            var drowned=new RealDrowned(world);((RandomSource)entityRandom.get(drowned)).setSeed(entitySeed);
            drowned.setUUID(new UUID(1,2));drowned.setPersistenceRequired();drowned.snapTo(new BlockPos(3,10,-2),0,0);
            var region=(ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)->switch(m.getName()){
                case "getRandom"->random;case "getBiome"->biome;case "getEntitiesOfClass"->List.of();
                case "registryAccess"->throw new StructureDrownedOracle.Stop("ENCHANT");default->throw new AssertionError(m);
            });
            drowned.finalizeSpawn(region,new StructureDrownedOracle.DifficultyValue(test.get("special").getAsFloat()),EntitySpawnReason.STRUCTURE,null);
            var output=TagValueOutput.createWithContext(ProblemReporter.DISCARDING,lookup);
            if(!drowned.save(output))throw new AssertionError("unexpected passenger");
            var actual=output.buildResult();var expected=NbtIo.read(Path.of(test.get("path").getAsString()));
            normalize(actual);normalize(expected);
            if(!actual.equals(expected))throw new AssertionError("case="+test+" java="+actual+" rust="+expected);
            if(random.nextLong()!=test.get("next_region").getAsLong())throw new AssertionError("region RNG "+test);
            if(((RandomSource)entityRandom.get(drowned)).nextLong()!=test.get("next_entity").getAsLong())throw new AssertionError("entity RNG "+test);
        }
        out.println("OK\t"+tests.size());
    }
}
