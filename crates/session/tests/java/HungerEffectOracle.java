import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.world.effect.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;

public class HungerEffectOracle {
  static class TestPlayer extends Player {
    float exhaustion; boolean invulnerable;
    TestPlayer() { super(null, null); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public void causeFoodExhaustion(float amount) {
      if (!invulnerable) exhaustion = Math.min(exhaustion + amount, 40.0f);
    }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe = (sun.misc.Unsafe)field.get(null);
    var player = (TestPlayer)unsafe.allocateInstance(TestPlayer.class);
    var rows = new JsonArray();
    for (int duration : new int[]{0,1,2,7,-1}) for (int amplifier : new int[]{0,1,3,255})
      for (float initial : new float[]{0.0f,3.99f,39.999f}) for (boolean invulnerable : new boolean[]{false,true}) {
        player.exhaustion = initial; player.invulnerable = invulnerable;
        var effect = new MobEffectInstance(MobEffects.HUNGER, duration, amplifier);
        var ticks = new JsonArray(); boolean active = true;
        for (int i=0; i<12; i++) {
          if (active) active = effect.tickServer(null, player, () -> {});
          var tick = new JsonObject(); tick.addProperty("exhaustion_bits", Float.floatToRawIntBits(player.exhaustion));
          tick.addProperty("active", active); if (active) tick.addProperty("duration", effect.getDuration()); ticks.add(tick);
        }
        var row = new JsonObject(); row.addProperty("duration",duration); row.addProperty("amplifier",amplifier);
        row.addProperty("initial",initial); row.addProperty("invulnerable",invulnerable); row.add("ticks",ticks); rows.add(row);
      }
    Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
  }
}
