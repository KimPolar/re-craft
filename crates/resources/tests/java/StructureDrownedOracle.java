import java.lang.reflect.Field;
import java.lang.reflect.Proxy;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BiomeTags;
import net.minecraft.util.RandomSource;
import net.minecraft.util.SpecialDates;
import net.minecraft.world.Difficulty;
import net.minecraft.world.DifficultyInstance;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.ai.attributes.Attributes;
import net.minecraft.world.entity.ai.navigation.AmphibiousPathNavigation;
import net.minecraft.world.entity.ai.navigation.PathNavigation;
import net.minecraft.world.entity.monster.zombie.Drowned;
import net.minecraft.world.entity.monster.zombie.Zombie;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.biome.Biomes;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

public class StructureDrownedOracle {
    static Object unsafe;
    static java.lang.reflect.Method allocate;
    static class Stop extends RuntimeException { final String event; Stop(String event){this.event=event;} }
    static class World extends ServerLevel {
        World(){ super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public int getNextEntityId(){return 1;}
        @Override public Difficulty getDifficulty(){return Difficulty.NORMAL;}
        @Override public FeatureFlagSet enabledFeatures(){
            for(var frame:Thread.currentThread().getStackTrace()) {
                if(frame.getClassName().equals(Zombie.class.getName()) && frame.getMethodName().equals("finalizeSpawn")) throw new Stop("CHICKEN");
                if(frame.getClassName().equals(Drowned.class.getName()) && frame.getMethodName().equals("finalizeSpawn")) throw new Stop("NAUTILUS");
            }
            throw new AssertionError("unexpected feature query");
        }
    }
    static class DifficultyValue extends DifficultyInstance {
        final float multiplier;
        DifficultyValue(float value){super(Difficulty.NORMAL,0L,0L,0f); multiplier=value;}
        @Override public float getSpecialMultiplier(){return multiplier;}
    }
    static class TestDrowned extends Drowned {
        boolean baby,doors;
        TestDrowned(World world){super(EntityTypes.DROWNED,world);}
        @Override protected void registerGoals(){}
        @Override protected PathNavigation createNavigation(Level level){
            try{return (PathNavigation)allocate.invoke(unsafe,AmphibiousPathNavigation.class);}
            catch(ReflectiveOperationException e){throw new RuntimeException(e);}
        }
        @Override public void setCanBreakDoors(boolean value){doors=value;}
        @Override public boolean canBreakDoors(){return doors;}
        @Override public void setBaby(boolean value){baby=value;}
        @Override public boolean isBaby(){return baby;}
    }
    static long bits(double x){return Double.doubleToLongBits(x);}
    static String modifier(net.minecraft.world.entity.ai.attributes.AttributeInstance instance, Identifier id){
        var value=instance.getModifier(id); return value==null?"-":Long.toString(bits(value.amount()));
    }
    public static void main(String[] args) throws Exception {
        var output=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registries=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries))pending.apply();
        var uc=Class.forName("sun.misc.Unsafe");var uf=uc.getDeclaredField("theUnsafe");uf.setAccessible(true);
        unsafe=uf.get(null);allocate=uc.getMethod("allocateInstance",Class.class);var world=(World)allocate.invoke(unsafe,World.class);
        var entityRandom=Entity.class.getDeclaredField("random");entityRandom.setAccessible(true);
        var leaderField=Zombie.class.getDeclaredField("LEADER_ZOMBIE_BONUS_ID");leaderField.setAccessible(true);
        var rangeField=Zombie.class.getDeclaredField("ZOMBIE_RANDOM_SPAWN_BONUS_ID");rangeField.setAccessible(true);
        var leader=(Identifier)leaderField.get(null);var range=(Identifier)rangeField.get(null);var spawn=Identifier.withDefaultNamespace("random_spawn_bonus");
        var biome=registries.lookupOrThrow(Registries.BIOME).getOrThrow(Biomes.OCEAN);
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",java.util.Collection.class);bind.setAccessible(true);
        boolean halloween=SpecialDates.isHalloween(); output.println("H\t"+halloween);
        float[] factors={0f,0.25f,0.5f,1f};
        for(int i=0;i<4096;i++){
            int algorithm=i%2;boolean frequent=(i&4)!=0;float factor=factors[(i/8)%4];
            long regionSeed=(long)i*671231L-12345L, entitySeed=(long)i*98317L+3329L;
            RandomSource random=algorithm==0?new LegacyRandomSource(regionSeed):new XoroshiroRandomSource(regionSeed);
            var drowned=new TestDrowned(world);var own=(RandomSource)entityRandom.get(drowned);own.setSeed(entitySeed);
            int[] chickenChecks={0},biomeChecks={0};
            bind.invoke(biome,frequent?List.of(BiomeTags.MORE_FREQUENT_DROWNED_SPAWNS):List.of());
            var region=(ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)->switch(m.getName()){
                case "getRandom" -> random;
                case "getEntitiesOfClass" -> { chickenChecks[0]++; yield List.of(); }
                case "getBiome" -> { biomeChecks[0]++; yield biome; }
                case "registryAccess" -> throw new Stop("ENCHANT");
                default -> throw new AssertionError("unexpected region call "+m);
            });
            String status="OK";
            try{drowned.finalizeSpawn(region,new DifficultyValue(factor),EntitySpawnReason.STRUCTURE,null);}
            catch(Stop stop){status=stop.event;}
            var reinf=drowned.getAttribute(Attributes.SPAWN_REINFORCEMENTS_CHANCE);
            var follow=drowned.getAttribute(Attributes.FOLLOW_RANGE);
            var knock=drowned.getAttribute(Attributes.KNOCKBACK_RESISTANCE);
            var max=drowned.getAttribute(Attributes.MAX_HEALTH);
            var main=drowned.getItemBySlot(EquipmentSlot.MAINHAND);var head=drowned.getItemBySlot(EquipmentSlot.HEAD);
            var off=drowned.getItemBySlot(EquipmentSlot.OFFHAND);
            output.println("Z\t"+i+"\t"+status+"\t"+chickenChecks[0]+"\t"+biomeChecks[0]+"\t"
                +random.nextLong()+"\t"+own.nextLong()+"\t"+drowned.isBaby()+"\t"+drowned.canPickUpLoot()+"\t"
                +drowned.canBreakDoors()+"\t"+drowned.isLeftHanded()+"\t"
                +(main.isEmpty()?"-":BuiltInRegistries.ITEM.getKey(main.getItem()))+"\t"
                +(head.isEmpty()?"-":BuiltInRegistries.ITEM.getKey(head.getItem()))+"\t"
                +off.is(Items.NAUTILUS_SHELL)+"\t"+modifier(follow,spawn)+"\t"+bits(reinf.getBaseValue())+"\t"
                +modifier(knock,spawn)+"\t"+modifier(follow,range)+"\t"+modifier(reinf,leader)+"\t"+modifier(max,leader)+"\t"
                +Float.floatToIntBits(drowned.getHealth()));
        }
    }
}
