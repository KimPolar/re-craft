import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.*;
import net.minecraft.server.dedicated.DedicatedServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.*;
import net.minecraft.world.item.enchantment.*;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;
import net.minecraft.tags.BlockTags;

public class LocationEnchantmentOracle {
    static class World extends ServerLevel {
        MinecraftServer server;
        RandomSource rng;
        BlockState state;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public MinecraftServer getServer() { return server; }
        @Override public RandomSource getRandom() { return rng; }
        @Override public BlockState getBlockState(BlockPos pos) { return state; }
        @Override public boolean hasChunkAt(BlockPos pos) { return true; }
        @Override public boolean isLoaded(BlockPos pos) { return true; }
    }
    static class TestPlayer extends Player {
        World world;
        boolean ground, passenger;
        Abilities abilities;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public ServerLevel level() { return world; }
        @Override public boolean onGround() { return ground; }
        @Override public boolean isFallFlying() { return false; }
        @Override public Abilities getAbilities() { return abilities; }
        @Override public Entity getVehicle() { return passenger ? this : null; }
        @Override public Vec3 position() { return new Vec3(0.5,1.0,0.5); }
        @Override public BlockPos getBlockPosBelowThatAffectsMyMovement() { return BlockPos.ZERO; }
    }
    static void set(Object target, Class<?> type, String name, Object value) throws Exception {
        var field=type.getDeclaredField(name); field.setAccessible(true); field.set(target,value);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup=VanillaRegistries.createWorldLookup();
        var bindTags=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bindTags.setAccessible(true);
        bindTags.invoke(Blocks.SOUL_SAND.builtInRegistryHolder(),List.of(BlockTags.SOUL_SPEED_BLOCKS));
        bindTags.invoke(Blocks.SOUL_SOIL.builtInRegistryHolder(),List.of(BlockTags.SOUL_SPEED_BLOCKS));
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
        var enchantment=lookup.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(Enchantments.SOUL_SPEED).value();
        var effects=enchantment.getEffects(EnchantmentEffectComponents.LOCATION_CHANGED);
        var method=Enchantment.class.getDeclaredMethod("locationContext",ServerLevel.class,int.class,Entity.class,boolean.class); method.setAccessible(true);
        var inputs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output=new JsonArray();
        for(var input:inputs) {
            var row=input.getAsJsonObject();
            player.ground=row.get("ground").getAsBoolean(); player.passenger=row.get("passenger").getAsBoolean();
            player.abilities.flying=row.get("flying").getAsBoolean();
            world.state=(row.get("block").getAsString().equals("soul_sand") ? Blocks.SOUL_SAND : Blocks.STONE).defaultBlockState();
            world.rng=new LegacyRandomSource(row.get("seed").getAsLong());
            var context=(net.minecraft.world.level.storage.loot.LootContext)method.invoke(null,world,row.get("level").getAsInt(),player,row.get("active").getAsBoolean());
            boolean matched=effects.get(row.get("index").getAsInt()).matches(context);
            var result=new JsonObject(); result.addProperty("matched",matched); result.addProperty("next",world.rng.nextInt()); output.add(result);
        }
        Files.writeString(Path.of(args[1]),new Gson().toJson(output));
    }
}
