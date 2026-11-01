import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.Holder;
import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.core.particles.ParticleTypes;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.util.RandomSource;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;
import net.minecraft.world.phys.Vec3;

public class PlayerDrownParticlesOracle {
    static class World extends ClientLevel {
        JsonArray particles;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public long getGameTime() { return 0; }
        @Override public void addParticle(ParticleOptions type,double x,double y,double z,double dx,double dy,double dz) {
            if(type != ParticleTypes.BUBBLE) throw new IllegalStateException("unexpected particle");
            var row=new JsonArray();
            for(double value:new double[]{x,y,z,dx,dy,dz}) row.add(Double.doubleToRawLongBits(value));
            particles.add(row);
        }
    }
    static class TestPlayer extends Player {
        World world; JsonArray sounds;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public ClientLevel level() { return world; }
        @Override public void playSound(SoundEvent sound,float volume,float pitch) {
            var row=new JsonObject(); row.addProperty("event",sound.location().toString());
            row.addProperty("pitch_bits",Float.floatToRawIntBits(pitch)); sounds.add(row);
        }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.world=world;
        var walk=LivingEntity.class.getDeclaredField("walkAnimation"); walk.setAccessible(true); walk.set(player,new WalkAnimationState());
        var position=Entity.class.getDeclaredField("position"); position.setAccessible(true);
        var velocity=Entity.class.getDeclaredField("deltaMovement"); velocity.setAccessible(true);
        var random=Entity.class.getDeclaredField("random"); random.setAccessible(true);
        var source=new DamageSource(Holder.direct(new DamageType("test",0.0f,DamageEffects.DROWNING)));
        var rows=new JsonArray();
        for(long seed:new long[]{0,1,37,1234567}) for(boolean hit:new boolean[]{false,true}) for(boolean dead:new boolean[]{false,true})
            for(double base:new double[]{0.0,42.25,-31.75}) {
                position.set(player,new Vec3(base,2.5,base+0.25)); velocity.set(player,new Vec3(0.2,-0.125,0.4));
                var rng=RandomSource.create(seed); random.set(player,rng); world.particles=new JsonArray(); player.sounds=new JsonArray();
                player.handleEntityEvent((byte)67); if(hit) player.handleDamageEvent(source); if(dead) player.handleEntityEvent((byte)3);
                var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("hit",hit); row.addProperty("dead",dead); row.addProperty("base",base);
                row.add("particles",world.particles); row.add("sounds",player.sounds); row.addProperty("next",rng.nextInt()); rows.add(row);
            }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
