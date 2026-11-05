import java.lang.reflect.Field;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.tags.BiomeTags;
import net.minecraft.world.level.biome.Biomes;
import net.minecraft.core.Holder;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.Difficulty;
import net.minecraft.world.DifficultyInstance;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntitySpawnReason;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.ai.attributes.Attributes;
import net.minecraft.world.entity.monster.zombie.Drowned;
import net.minecraft.world.entity.monster.zombie.Zombie;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.entity.ai.navigation.PathNavigation;
import net.minecraft.world.entity.ai.navigation.AmphibiousPathNavigation;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

public class DrownedSpawnOracle {
    static Object unsafe;
    static java.lang.reflect.Method allocate;
    static class World extends ServerLevel {
        int featureChecks;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public int getNextEntityId() { return 1; }
        @Override public Difficulty getDifficulty() { return Difficulty.NORMAL; }
        @Override public FeatureFlagSet enabledFeatures() { featureChecks++; throw new AbortNautilus(); }
    }
    static class AbortNautilus extends RuntimeException {}
    static class TestDrowned extends Drowned {
        boolean canBreak;
        boolean suppressZombieHooks;
        boolean testBaby;
        TestDrowned(World world) { super(EntityTypes.DROWNED,world); }
        @Override protected void registerGoals() {}
        @Override protected PathNavigation createNavigation(Level level) {
            try { return (PathNavigation)allocate.invoke(unsafe,AmphibiousPathNavigation.class); }
            catch (ReflectiveOperationException error) { throw new RuntimeException(error); }
        }
        // AmphibiousPathNavigation.canNavigateGround() is true in 26.3.
        @Override public void setCanBreakDoors(boolean value) { canBreak=value; }
        @Override public boolean canBreakDoors() { return canBreak; }
        @Override public boolean isBaby() { return testBaby; }
        @Override protected void populateDefaultEquipmentSlots(RandomSource random, DifficultyInstance difficulty) {
            if(!suppressZombieHooks) super.populateDefaultEquipmentSlots(random,difficulty);
        }
        @Override protected void populateDefaultEquipmentEnchantments(ServerLevelAccessor world, RandomSource random, DifficultyInstance difficulty) {
            if(!suppressZombieHooks) super.populateDefaultEquipmentEnchantments(world,random,difficulty);
        }
        @Override protected void handleAttributes(float multiplier, EntitySpawnReason reason) {
            if(!suppressZombieHooks) super.handleAttributes(multiplier,reason);
        }
        void equip(RandomSource random) { populateDefaultEquipmentSlots(random,null); }
        void attributes(float multiplier) { handleAttributes(multiplier,EntitySpawnReason.STRUCTURE); }
    }
    static long bits(double v) { return Double.doubleToLongBits(v); }
    public static void main(String[] args) throws Exception {
        var output=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registries=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var unsafeClass=Class.forName("sun.misc.Unsafe"); var unsafeField=unsafeClass.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        unsafe=unsafeField.get(null); allocate=unsafeClass.getMethod("allocateInstance",Class.class);
        var world=(World)allocate.invoke(unsafe,World.class);
        Field entityRandom=Entity.class.getDeclaredField("random"); entityRandom.setAccessible(true);
        var leaderField=Zombie.class.getDeclaredField("LEADER_ZOMBIE_BONUS_ID"); leaderField.setAccessible(true);
        var rangeField=Zombie.class.getDeclaredField("ZOMBIE_RANDOM_SPAWN_BONUS_ID"); rangeField.setAccessible(true);
        var leaderId=(Identifier)leaderField.get(null); var rangeId=(Identifier)rangeField.get(null);
        var knockId=Identifier.withDefaultNamespace("random_spawn_bonus");
        var equipment=new TestDrowned(world);
        for(int algorithm=0;algorithm<2;algorithm++) for(int i=0;i<4096;i++) {
            long seed=(long)i*1234567L-991L;
            RandomSource random=algorithm==0?new LegacyRandomSource(seed):new XoroshiroRandomSource(seed);
            equipment.setItemSlot(EquipmentSlot.MAINHAND,new ItemStack(Items.STICK));
            equipment.equip(random);
            output.println("E\t"+algorithm+"\t"+i+"\t"+BuiltInRegistries.ITEM.getKey(equipment.getMainHandItem().getItem())+"\t"+random.nextLong());
        }
        float[] multipliers={0.0f,0.25f,0.5f,1.0f};
        for(int i=0;i<1024;i++) {
            long seed=(long)i*7342347L-763L; float multiplier=multipliers[i%4];
            var drowned=new TestDrowned(world); var random=(RandomSource)entityRandom.get(drowned); random.setSeed(seed);
            drowned.attributes(multiplier);
            var reinf=drowned.getAttribute(Attributes.SPAWN_REINFORCEMENTS_CHANCE);
            var knock=drowned.getAttribute(Attributes.KNOCKBACK_RESISTANCE).getModifier(knockId);
            var follow=drowned.getAttribute(Attributes.FOLLOW_RANGE).getModifier(rangeId);
            var leaderReinf=reinf.getModifier(leaderId);
            var leaderHealth=drowned.getAttribute(Attributes.MAX_HEALTH).getModifier(leaderId);
            output.println("A\t"+i+"\t"+bits(reinf.getBaseValue())+"\t"+bits(knock.amount())+"\t"
                +(follow==null?"-":Long.toString(bits(follow.amount())))+"\t"
                +(leaderReinf==null?"-":Long.toString(bits(leaderReinf.amount())))+"\t"
                +(leaderHealth==null?"-":Long.toString(bits(leaderHealth.amount())))+"\t"
                +drowned.canBreakDoors()+"\t"+Float.floatToIntBits(drowned.getHealth())+"\t"+random.nextLong());
        }
        var difficulty=new DifficultyInstance(Difficulty.NORMAL,0L,0L,0.0f);
        var biome=registries.lookupOrThrow(Registries.BIOME).getOrThrow(Biomes.OCEAN);
        var bindBiome=Holder.Reference.class.getDeclaredMethod("bindTags",java.util.Collection.class); bindBiome.setAccessible(true);
        var reasons=new EntitySpawnReason[]{EntitySpawnReason.NATURAL,EntitySpawnReason.STRUCTURE,EntitySpawnReason.COMMAND};
        for(int i=0;i<2048;i++) {
            final boolean frequent=(i&8)!=0;
            boolean empty=(i&1)!=0, trident=(i&2)!=0, baby=(i&4)!=0;
            int algorithm=(i/48)%2; int reason=(i/16)%3;
            long seed=(long)i*104729L-3701L;
            RandomSource random=algorithm==0?new LegacyRandomSource(seed):new XoroshiroRandomSource(seed);
            int[] biomeChecks={0};
            bindBiome.invoke(biome,frequent?List.of(BiomeTags.MORE_FREQUENT_DROWNED_SPAWNS):List.of());
            var region=(ServerLevelAccessor)java.lang.reflect.Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)->{
                if(m.getName().equals("getRandom")) return random;
                if(m.getName().equals("getBiome")) { biomeChecks[0]++; return biome; }
                throw new AssertionError("unexpected region call "+m);
            });
            var drowned=new TestDrowned(world); drowned.suppressZombieHooks=true; drowned.testBaby=baby;
            drowned.setItemSlot(EquipmentSlot.HEAD,new ItemStack(Items.STICK));
            drowned.setItemSlot(EquipmentSlot.MAINHAND,new ItemStack(trident?Items.TRIDENT:Items.STICK));
            drowned.setItemSlot(EquipmentSlot.OFFHAND,empty?ItemStack.EMPTY:new ItemStack(Items.STICK));
            world.featureChecks=0;
            try { drowned.finalizeSpawn(region,difficulty,reasons[reason],new Zombie.ZombieGroupData(false,false)); }
            catch(AbortNautilus expected) {}
            boolean shell=drowned.getItemBySlot(EquipmentSlot.OFFHAND).is(Items.NAUTILUS_SHELL);
            output.println("P\t"+i+"\t"+shell+"\t"+biomeChecks[0]+"\t"+world.featureChecks+"\t"+random.nextLong());
        }
    }
}
