import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.effect.*;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;

public class ConcurrentHealthEffectsOracle {
  static class TestPlayer extends Player {
    float health, absorption, maximum;
    boolean hurt;
    DamageSources sources;
    TestPlayer() { super(null, null); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public float getHealth() { return health; }
    @Override public double getAttributeValue(Holder<Attribute> attribute) { return maximum; }
    @Override public DamageSources damageSources() { return sources; }
    @Override public void heal(float amount) { if (health > 0) health = Math.min(health + amount, maximum); }
    @Override public boolean hurtServer(ServerLevel level, DamageSource source, float amount) {
      hurt = true;
      float absorbed = Math.max(0, Math.min(absorption, amount));
      absorption -= absorbed;
      health = Math.max(0, health - (amount - absorbed));
      return true;
    }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion();
    net.minecraft.server.Bootstrap.bootStrap();
    var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
    field.setAccessible(true);
    var unsafe = (sun.misc.Unsafe) field.get(null);
    var player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
    player.sources = (DamageSources) unsafe.allocateInstance(DamageSources.class);
    var rows = new JsonArray();
    for (float health : new float[]{0.5f,1f,1.5f,2f,5f,10f,19f,19.5f,20f})
      for (float absorption : new float[]{0f,0.25f,1f,2f}) {
        var row = new JsonObject();
        row.addProperty("health", health);
        row.addProperty("absorption", absorption);
        var orders = new JsonArray();
        for (boolean poisonFirst : new boolean[]{true,false}) {
          player.health = health; player.absorption = absorption; player.maximum = 20f;
          player.hurt = false; player.tickCount = 1;
          var poison = new MobEffectInstance(MobEffects.POISON, 50, 0);
          var regeneration = new MobEffectInstance(MobEffects.REGENERATION, 50, 0);
          if (poisonFirst) {
            poison.tickServer(null, player, () -> {});
            regeneration.tickServer(null, player, () -> {});
          } else {
            regeneration.tickServer(null, player, () -> {});
            poison.tickServer(null, player, () -> {});
          }
          var result = new JsonObject();
          result.addProperty("health_bits", Float.floatToRawIntBits(player.health));
          result.addProperty("absorption_bits", Float.floatToRawIntBits(player.absorption));
          result.addProperty("hurt", player.hurt);
          orders.add(result);
        }
        row.add("orders", orders);
        rows.add(row);
      }
    Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
  }
}
