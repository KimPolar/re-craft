import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.effect.*;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;

public class RegenerationEffectOracle {
  static class TestPlayer extends Player {
    float health, maximum;
    TestPlayer() { super(null, null); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public float getHealth() { return health; }
    @Override public double getAttributeValue(Holder<Attribute> attribute) { return maximum; }
    @Override public void heal(float amount) { if(health>0.0f) health=Math.min(health+amount,maximum); }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
    var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); var rows=new JsonArray();
    for(int duration:new int[]{0,1,2,24,25,26,49,50,51,100,-1})
      for(int amplifier:new int[]{0,1,2,4,5,31,32,255})
        for(float maximum:new float[]{3.5f,20.0f,60.0f})
        for(float health:new float[]{0.0f,1.0f,1.5f,19.5f,20.0f,21.0f}) {
          player.health=health; player.maximum=maximum; player.tickCount=0;
          var effect=new MobEffectInstance(MobEffects.REGENERATION,duration,amplifier);
          var ticks=new JsonArray(); boolean active=true;
          for(int i=0;i<30;i++) {
            player.tickCount++; if(active)active=effect.tickServer((ServerLevel)null,player,()->{});
            var tick=new JsonObject();tick.addProperty("health_bits",Float.floatToRawIntBits(player.health));
            tick.addProperty("active",active);if(active)tick.addProperty("duration",effect.getDuration());ticks.add(tick);
          }
          var row=new JsonObject();row.addProperty("duration",duration);row.addProperty("amplifier",amplifier);row.addProperty("health",health);row.addProperty("maximum",maximum);row.add("ticks",ticks);rows.add(row);
        }
    Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
  }
}
