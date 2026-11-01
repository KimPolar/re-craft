import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.sounds.SoundSource;
import net.minecraft.util.RandomSource;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.phys.Vec3;

public class PlayerLavaSoundOracle {
    static class World extends ServerLevel {
        boolean called; float volume, pitch; String sound, category;
        double x,y,z;
        World() { super(null,null,null,null,null,null,false,0L,null,false); }
        @Override public void playSound(Entity source,double x,double y,double z,SoundEvent event,SoundSource category,float volume,float pitch) {
            if(source != null) throw new AssertionError("unexpected excluded player");
            called=true; this.x=x; this.y=y; this.z=z; this.sound=event.location().toString();
            this.category=category.getName(); this.volume=volume; this.pitch=pitch;
        }
    }
    static class Player extends ServerPlayer {
        World world; DamageSources sources; boolean accepted, silent;
        Player() { super(null,null,null,null); }
        @Override public ServerLevel level() { return world; }
        @Override public DamageSources damageSources() { return sources; }
        @Override public boolean fireImmune() { return false; }
        @Override public boolean isSilent() { return silent; }
        @Override public boolean hurtServer(ServerLevel world,DamageSource source,float amount) {
            if(amount != 4.0f || source != sources.lava()) throw new AssertionError("unexpected lava damage");
            return accepted;
        }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var player=(Player)unsafe.allocateInstance(Player.class); player.world=world;
        var sources=(DamageSources)unsafe.allocateInstance(DamageSources.class);
        var lava=DamageSources.class.getDeclaredField("lava"); lava.setAccessible(true); lava.set(sources,new DamageSource(Holder.direct(new DamageType("test",0.0f)))); player.sources=sources;
        var pos=Entity.class.getDeclaredField("position"); pos.setAccessible(true); pos.set(player,new Vec3(3.5,7.25,-4.5));
        var randomField=Entity.class.getDeclaredField("random"); randomField.setAccessible(true);
        var rows=new JsonArray();
        for(boolean accepted:new boolean[]{false,true}) for(boolean silent:new boolean[]{false,true}) for(long seed:new long[]{0,1,37,1234567}) {
            player.accepted=accepted; player.silent=silent; world.called=false;
            var random=RandomSource.create(seed); randomField.set(player,random);
            player.lavaHurt();
            var row=new JsonObject(); row.addProperty("accepted",accepted); row.addProperty("silent",silent); row.addProperty("seed",seed); row.addProperty("called",world.called);
            if(world.called) { row.addProperty("sound",world.sound); row.addProperty("category",world.category); row.addProperty("x",world.x); row.addProperty("y",world.y); row.addProperty("z",world.z); row.addProperty("volume_bits",Float.floatToRawIntBits(world.volume)); row.addProperty("pitch_bits",Float.floatToRawIntBits(world.pitch)); }
            row.addProperty("next",random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
