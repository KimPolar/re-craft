import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.util.RandomSource;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.WalkAnimationState;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;

public class PlayerClientDamageSoundOracle {
    static class World extends ServerLevel {
        World() { super(null,null,null,null,null,null,false,0L,null,false); }
        @Override public long getGameTime() { return 0; }
    }
    static class TestPlayer extends Player {
        World world; boolean silent; JsonArray sounds;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public ServerLevel level() { return world; }
        @Override public boolean isSilent() { return silent; }
        @Override public void playSound(SoundEvent sound,float volume,float pitch) {
            var entry=new JsonObject(); entry.addProperty("sound",sound.location().toString());
            entry.addProperty("category",getSoundSource().getName()); entry.addProperty("volume_bits",Float.floatToRawIntBits(volume));
            entry.addProperty("pitch_bits",Float.floatToRawIntBits(pitch)); sounds.add(entry);
        }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.world=world;
        var walk=LivingEntity.class.getDeclaredField("walkAnimation"); walk.setAccessible(true); walk.set(player,new WalkAnimationState());
        var randomField=Entity.class.getDeclaredField("random"); randomField.setAccessible(true);
        var source=new DamageSource(Holder.direct(new DamageType("test",0.1f,DamageEffects.BURNING)));
        var rows=new JsonArray();
        for(boolean silent:new boolean[]{false,true}) for(boolean dead:new boolean[]{false,true}) for(long seed:new long[]{0,1,37,1234567}) {
            player.silent=silent; player.sounds=new JsonArray(); var random=RandomSource.create(seed); randomField.set(player,random);
            player.handleDamageEvent(source); if(dead) player.handleEntityEvent((byte)3);
            var row=new JsonObject(); row.addProperty("silent",silent); row.addProperty("dead",dead); row.addProperty("seed",seed);
            row.add("sounds",player.sounds); row.addProperty("next",random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
