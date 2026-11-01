import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.client.renderer.LevelEventHandler;
import net.minecraft.core.BlockPos;
import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.sounds.SoundSource;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;

public class LavaFizzOracle {
  static class World extends ClientLevel {
    RandomSource rng; JsonArray particles; float volume, pitch; String sound, category;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public RandomSource getRandom() { return rng; }
    @Override public void playLocalSound(BlockPos pos, SoundEvent sound, SoundSource category, float volume, float pitch, boolean delayed) {
      this.sound = sound.location().toString(); this.category = category.getName(); this.volume = volume; this.pitch = pitch;
    }
    @Override public void addParticle(ParticleOptions type, double x, double y, double z, double vx, double vy, double vz) {
      var row=new JsonArray(); row.add(Double.doubleToRawLongBits(x)); row.add(Double.doubleToRawLongBits(y)); row.add(Double.doubleToRawLongBits(z)); particles.add(row);
    }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null); var world=(World)unsafe.allocateInstance(World.class);
    var handler=new LevelEventHandler(null,world); var out=new JsonArray();
    for(long seed:new long[]{0,1,42,-99,123456789}) {
      world.rng=new LegacyRandomSource(seed); world.particles=new JsonArray();
      handler.levelEvent(1501,new BlockPos(17,-20,-43),0);
      var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("sound",world.sound); row.addProperty("category",world.category);
      row.addProperty("volume",Float.floatToRawIntBits(world.volume)); row.addProperty("pitch",Float.floatToRawIntBits(world.pitch));
      row.add("particles",world.particles); row.addProperty("next",world.rng.nextInt()); out.add(row);
    }
    Files.writeString(Path.of(args[0]),out.toString());
  }
}
