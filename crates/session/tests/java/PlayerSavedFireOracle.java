import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.TagKey;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.*;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.material.Fluid;

public class PlayerSavedFireOracle {
    static class World extends ServerLevel {
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
    }
    static class TestPlayer extends Player {
        DamageSources sources; int hits;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override protected void computeSpeed() { }
        @Override public boolean isPassenger() { return false; }
        @Override protected void handlePortal() { }
        @Override public boolean canSpawnSprintParticle() { return false; }
        @Override public boolean isEyeInFluid(TagKey<Fluid> tag) { return false; }
        @Override protected boolean updateFluidInteraction() { return false; }
        @Override public void updateSwimming() { }
        @Override public boolean fireImmune() { return false; }
        @Override public boolean isInLava() { return false; }
        @Override public void checkBelowWorld() { }
        @Override public void setSharedFlagOnFire(boolean value) { }
        @Override public DamageSources damageSources() { return sources; }
        @Override public boolean hurtServer(ServerLevel level, DamageSource source, float amount) { hits++; return true; }
    }
    public static void main(String[] args) throws Throwable {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        player.sources=(DamageSources)unsafe.allocateInstance(DamageSources.class);
        var level=Entity.class.getDeclaredField("level"); level.setAccessible(true); level.set(player,world);
        var abilities=Player.class.getDeclaredField("abilities"); abilities.setAccessible(true); abilities.set(player,new Abilities());
        var fire=Entity.class.getDeclaredField("remainingFireTicks"); fire.setAccessible(true);
        var lookup=java.lang.invoke.MethodHandles.privateLookupIn(Entity.class,java.lang.invoke.MethodHandles.lookup());
        var baseTick=lookup.findSpecial(Entity.class,"baseTick",java.lang.invoke.MethodType.methodType(void.class),Entity.class).bindTo(player);
        var rows=new JsonArray();
        for(int initial:new int[]{2,19,20,21,39,40,41,200}) {
            fire.setInt(player,initial); player.hits=0; baseTick.invokeExact();
            var row=new JsonObject(); row.addProperty("initial",initial);
            row.addProperty("fire",player.getRemainingFireTicks()); row.addProperty("hits",player.hits); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
