import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.sounds.SoundSource;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.phys.Vec3;

public class PlayerExtinguishSoundOracle {
    static class World extends ServerLevel {
        float volume, pitch;
        String sound, category;
        double x, y, z;
        boolean called;
        World() { super(null,null,null,null,null,null,false,0L,null,false); }
        @Override public void playSound(Entity source, double x, double y, double z, SoundEvent event, SoundSource category, float volume, float pitch) {
            if (source != null) throw new AssertionError("unexpected excluded player");
            this.called = true; this.x = x; this.y = y; this.z = z;
            this.sound = event.location().toString(); this.category = category.getName();
            this.volume = volume; this.pitch = pitch;
        }
    }
    static class Player extends ServerPlayer {
        World world; boolean silent;
        Player() { super(null,null,null,null); }
        @Override public ServerLevel level() { return world; }
        @Override public boolean isSilent() { return silent; }
        void extinguishSound() { playEntityOnFireExtinguishedSound(); }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var player=(Player)unsafe.allocateInstance(Player.class);
        player.world=world;
        var levelField=Entity.class.getDeclaredField("level"); levelField.setAccessible(true); levelField.set(player,world);
        var posField=Entity.class.getDeclaredField("position"); posField.setAccessible(true); posField.set(player,new Vec3(3.5, 7.25, -4.5));
        var randomField=Entity.class.getDeclaredField("random"); randomField.setAccessible(true);
        var rows=new JsonArray();
        for(boolean silent:new boolean[]{false,true}) for(long seed:new long[]{0,1,37,1234567}) {
            player.silent=silent;
            var random=RandomSource.create(seed); randomField.set(player,random); world.called=false;
            player.extinguishSound();
            if(!world.called) throw new AssertionError("no extinguish sound");
            var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("silent",silent);
            row.addProperty("sound",world.sound); row.addProperty("category",world.category);
            row.addProperty("x",world.x); row.addProperty("y",world.y); row.addProperty("z",world.z);
            row.addProperty("volume_bits",Float.floatToRawIntBits(world.volume));
            row.addProperty("pitch_bits",Float.floatToRawIntBits(world.pitch));
            row.addProperty("next",random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
