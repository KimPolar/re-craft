import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.effect.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;

public class PoisonEffectOracle {
  static class TestPlayer extends Player {
    float health; boolean hit; DamageSources sources;
    TestPlayer() { super(null, null); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public float getHealth() { return health; }
    @Override public DamageSources damageSources() { return sources; }
    @Override public boolean hurtServer(ServerLevel level, DamageSource source, float amount) {
      hit = true; health -= amount; return true;
    }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe = (sun.misc.Unsafe) field.get(null);
    var player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
    player.sources = (DamageSources) unsafe.allocateInstance(DamageSources.class);
    var rows = new JsonArray();
    for (int duration : new int[]{0, 1, 2, 24, 25, 26, 50, -1})
      for (int amplifier : new int[]{0, 1, 2, 4, 5, 31, 32, 255})
        for (float health : new float[]{1.0f, 1.5f, 5.0f, 20.0f}) {
          player.health = health; player.tickCount = 0;
          var effect = new MobEffectInstance(MobEffects.POISON, duration, amplifier);
          var ticks = new JsonArray(); boolean active = true;
          for (int i = 0; i < 30; i++) {
            player.tickCount++; player.hit = false;
            if (active) active = effect.tickServer(null, player, () -> {});
            var tick = new JsonObject(); tick.addProperty("hit", player.hit);
            tick.addProperty("health_bits", Float.floatToRawIntBits(player.health));
            tick.addProperty("active", active);
            if (active) tick.addProperty("duration", effect.getDuration());
            ticks.add(tick);
          }
          var row = new JsonObject(); row.addProperty("duration", duration); row.addProperty("amplifier", amplifier);
          row.addProperty("health", health); row.add("ticks", ticks); rows.add(row);
        }
    Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
  }
}
