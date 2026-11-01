import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.*;
import net.minecraft.server.dedicated.DedicatedServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.sounds.*;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.player.*;
import net.minecraft.world.item.*;
import net.minecraft.world.item.enchantment.*;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;

public class TickEnchantmentOracle {
    static class World extends ServerLevel {
        MinecraftServer server; RandomSource rng; BlockState state; JsonArray events;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public MinecraftServer getServer() { return server; }
        @Override public RandomSource getRandom() { return rng; }
        @Override public BlockState getBlockState(BlockPos pos) { return state; }
        @Override public boolean isLoaded(BlockPos pos) { return true; }
        @Override public <T extends ParticleOptions> int sendParticles(T particle,double x,double y,double z,int count,double vx,double vy,double vz,double speed) {
            var row=new JsonObject(); row.addProperty("kind","particle");
            var pos=new JsonArray(); for(double v:new double[]{x,y,z}) pos.add(Double.doubleToRawLongBits(v)); row.add("position",pos);
            var vel=new JsonArray(); for(double v:new double[]{vx,vy,vz}) vel.add(Double.doubleToRawLongBits(v)); row.add("velocity",vel);
            row.addProperty("speed",speed); row.addProperty("count",count); events.add(row); return 1;
        }
        @Override public void playSound(Entity excluded,double x,double y,double z,Holder<SoundEvent> sound,SoundSource source,float volume,float pitch) {
            var row=new JsonObject(); row.addProperty("kind","sound"); row.addProperty("id",sound.value().location().toString());
            row.addProperty("volume",volume); row.addProperty("pitch",pitch); events.add(row);
        }
    }
    static class TestPlayer extends Player {
        World world; boolean ground,silent; Abilities abilities; Vec3 movement; RandomSource rng;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public ServerLevel level() { return world; }
        @Override public boolean onGround() { return ground; }
        @Override public boolean isFallFlying() { return false; }
        @Override public Abilities getAbilities() { return abilities; }
        @Override public boolean isSilent() { return silent; }
        @Override public RandomSource getRandom() { return rng; }
        @Override public Vec3 getKnownMovement() { return movement; }
        @Override public Vec3 position() { return new Vec3(8.5,1.875,8.5); }
        @Override public BlockPos getBlockPosBelowThatAffectsMyMovement() { return BlockPos.ZERO; }
    }
    static void set(Object target,Class<?> type,String name,Object value) throws Exception {
        var f=type.getDeclaredField(name); f.setAccessible(true); f.set(target,value);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup=VanillaRegistries.createWorldLookup();
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
        bind.invoke(Blocks.SOUL_SAND.builtInRegistryHolder(),List.of(BlockTags.SOUL_SPEED_BLOCKS));
        bind.invoke(Blocks.SOUL_SOIL.builtInRegistryHolder(),List.of(BlockTags.SOUL_SPEED_BLOCKS));
        var reloadable=VanillaRegistries.createReloadableLookup(lookup);
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) pending.apply();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var server=(DedicatedServer)unsafe.allocateInstance(DedicatedServer.class);
        var managers=(ReloadableServerResources)unsafe.allocateInstance(ReloadableServerResources.class);
        set(managers,ReloadableServerResources.class,"fullRegistryHolder",new ReloadableServerRegistries.Holder(reloadable));
        var resources=Class.forName("net.minecraft.server.MinecraftServer$ReloadableResources");
        var constructor=resources.getDeclaredConstructors()[0]; constructor.setAccessible(true);
        set(server,MinecraftServer.class,"resources",constructor.newInstance(null,managers));
        var world=(World)unsafe.allocateInstance(World.class); world.server=server;
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.world=world; player.abilities=new Abilities();
        set(player,Entity.class,"dimensions",EntityDimensions.fixed(0.6f,1.8f));
        var soul=lookup.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(Enchantments.SOUL_SPEED);
        var item=new EnchantedItemInUse(new ItemStack(Items.IRON_BOOTS),EquipmentSlot.FEET,null,ignored -> {});
        var inputs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray(); var results=new JsonArray();
        for(var input:inputs) {
            var row=input.getAsJsonObject(); player.ground=row.get("ground").getAsBoolean(); player.silent=row.get("silent").getAsBoolean();
            player.abilities.flying=row.get("flying").getAsBoolean(); player.tickCount=row.get("tick").getAsInt();
            player.movement=new Vec3(row.get("vx").getAsDouble(),0.0,row.get("vz").getAsDouble());
            player.rng=new LegacyRandomSource(row.get("entity_seed").getAsLong()); world.rng=new LegacyRandomSource(row.get("level_seed").getAsLong());
            world.state=(row.get("soul").getAsBoolean()?Blocks.SOUL_SAND:Blocks.STONE).defaultBlockState(); world.events=new JsonArray();
            soul.value().tick(world,row.get("level").getAsInt(),item,player);
            var output=new JsonObject(); output.add("events",world.events); output.addProperty("level_next",world.rng.nextInt()); output.addProperty("entity_next",player.rng.nextInt()); results.add(output);
        }
        Files.writeString(Path.of(args[1]),new Gson().toJson(results));
    }
}
