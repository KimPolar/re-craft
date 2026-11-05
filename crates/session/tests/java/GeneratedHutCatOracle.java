import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.*;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.animal.feline.*;
import net.minecraft.world.entity.variant.SpawnPrioritySelectors;
import net.minecraft.world.entity.variant.SpawnContext;
import net.minecraft.world.entity.variant.MoonBrightnessCheck;
import net.minecraft.advancements.predicates.MinMaxBounds;
import net.minecraft.world.attribute.*;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.level.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.structures.SwampHutPiece;
import net.minecraft.world.level.levelgen.structure.pieces.PiecesContainer;
import net.minecraft.world.level.storage.TagValueOutput;
import com.mojang.serialization.Lifecycle;
import sun.misc.Unsafe;

public class GeneratedHutCatOracle {
    static RegistryAccess access;
    static class World extends ServerLevel {
        World(){super(null,null,null,null,null,null,false,0L,List.of(),false);}
        @Override public int getNextEntityId(){return 1;}
        @Override public RegistryAccess registryAccess(){return access;}
    }
    static class TestCat extends Cat {
        TestCat(World world){super(EntityTypes.CAT,world);}
        @Override protected net.minecraft.world.entity.ai.navigation.PathNavigation createNavigation(Level level){return null;}
        @Override protected void registerGoals(){}
    }
    public static void main(String[] args)throws Exception {
        SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var field=Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);
        var unsafe=(Unsafe)field.get(null);var world=(World)unsafe.allocateInstance(World.class);
        var lookup=VanillaRegistries.createWorldLookup();
        var input=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        for(var entry:input){
            var test=entry.getAsJsonObject();String mode=test.get("mode").getAsString();
            var variants=new MappedRegistry<CatVariant>(Registries.CAT_VARIANT,Lifecycle.stable());
            for(var name:List.of("all_black","black")){
                var key=ResourceKey.create(Registries.CAT_VARIANT,Identifier.withDefaultNamespace(name));
                var base=lookup.lookupOrThrow(Registries.CAT_VARIANT).getOrThrow(key).value();
                var selectors=mode.equals("none")?SpawnPrioritySelectors.EMPTY:SpawnPrioritySelectors.fallback(mode.equals("high")&&name.equals("all_black")?1:0);
                variants.register(key,new CatVariant(base.adultAssetInfo(),base.babyAssetInfo(),selectors),RegistrationInfo.BUILT_IN);
            }
            var sounds=new MappedRegistry<CatSoundVariant>(Registries.CAT_SOUND_VARIANT,Lifecycle.stable());
            for(var name:List.of("classic","royal")){
                var key=ResourceKey.create(Registries.CAT_SOUND_VARIANT,Identifier.withDefaultNamespace(name));
                sounds.register(key,lookup.lookupOrThrow(Registries.CAT_SOUND_VARIANT).getOrThrow(key).value(),RegistrationInfo.BUILT_IN);
            }
            access=new RegistryAccess.ImmutableRegistryAccess(List.of(variants.freeze(),sounds.freeze()));
            long seed=test.get("seed").getAsLong();RandomSource root=test.get("legacy").getAsBoolean()?new LegacyRandomSource(seed):new XoroshiroRandomSource(seed);
            var random=root.forkPositional().fromHashOf(Identifier.withDefaultNamespace("worldgen_region_random")).forkPositional().at(-13<<4,0,17<<4);
            var region=(ServerLevelAccessor)java.lang.reflect.Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)->switch(m.getName()){
                case "getRandom" -> random; case "getBiome","environmentAttributes" -> null;case "registryAccess" -> access;default -> throw new AssertionError(m);
            });
            var cat=new TestCat(world);cat.setUUID(new UUID(1,2));cat.setPersistenceRequired();cat.snapTo(3.5,10,-2.5,0,0);
            cat.finalizeSpawn(region,null,EntitySpawnReason.STRUCTURE,null);
            var output=TagValueOutput.createWithContext(ProblemReporter.DISCARDING,lookup);
            if(!cat.save(output))throw new AssertionError("cat not saved");
            var actual=output.buildResult();var expected=NbtIo.read(Path.of(test.get("path").getAsString()));
            if(!actual.equals(expected))throw new AssertionError("case="+test+" java="+actual+" rust="+expected);
            if(random.nextLong()!=test.get("next").getAsLong())throw new AssertionError("random stream differs: "+test);
        }
        for(var entry:input.get(0).getAsJsonObject().getAsJsonArray("moon_checks")) {
            var check=entry.getAsJsonObject();var phase=MoonPhase.values()[check.get("phase").getAsInt()];
            var reader=new EnvironmentAttributeReader(){@Override @SuppressWarnings("unchecked") public <T> T getValue(EnvironmentAttribute<T> attribute,Vec3 point,SpatialAttributeInterpolator interpolator){return (T)phase;} @Override @SuppressWarnings("unchecked") public <T> T getDimensionValue(EnvironmentAttribute<T> attribute){return (T)phase;}};
            var context=new SpawnContext(new BlockPos(3,10,-3),null,reader,null);
            var predicate=new MoonBrightnessCheck(MinMaxBounds.Doubles.between(check.get("min").getAsDouble(),check.get("max").getAsDouble()));
            if(predicate.test(context)!=check.get("expected").getAsBoolean())throw new AssertionError("moon condition differs: "+check);
        }
        var piece=new SwampHutPiece(RandomSource.create(42),0,0);
        var boxField=StructurePiece.class.getDeclaredField("boundingBox");boxField.setAccessible(true);
        boxField.set(piece,new BoundingBox(3,10,-3,9,16,5));
        var start=new StructureStart(null,new ChunkPos(0,0),0,new PiecesContainer(List.of(piece)));
        var manager=(StructureManager)unsafe.allocateInstance(StructureManager.class);
        for(var entry:input.get(0).getAsJsonObject().getAsJsonArray("piece_checks")) {
            var check=entry.getAsJsonObject();var point=new BlockPos(check.get("x").getAsInt(),check.get("y").getAsInt(),check.get("z").getAsInt());
            if(manager.structureHasPieceAt(point,start)!=check.get("expected").getAsBoolean())throw new AssertionError("structure piece condition differs: "+check);
        }
    }
}
