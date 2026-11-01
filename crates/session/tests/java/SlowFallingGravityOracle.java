import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;
import net.minecraft.world.phys.Vec3;

public class SlowFallingGravityOracle {
  static class TestPlayer extends Player {
    double gravity, vertical;
    boolean effect, noGravity;
    TestPlayer() { super(null,null); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public Vec3 getDeltaMovement() { return new Vec3(0,vertical,0); }
    @Override public boolean hasEffect(Holder<MobEffect> ignored) { return effect; }
    @Override public boolean isNoGravity() { return noGravity; }
    @Override protected double getDefaultGravity() { return gravity; }
    double effective() { return getEffectiveGravity(); }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null);
    var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
    var rows=new JsonArray();
    for(double gravity:new double[]{-0.2,0.0,0.005,0.01,0.08,0.3})
      for(double vertical:new double[]{-1.0,-0.001,-0.0,0.0,0.001,1.0})
        for(boolean effect:new boolean[]{false,true})
          for(boolean noGravity:new boolean[]{false,true}) {
            player.gravity=gravity; player.vertical=vertical; player.effect=effect; player.noGravity=noGravity;
            var row=new JsonObject();
            row.addProperty("gravity",gravity); row.addProperty("vertical",vertical);
            row.addProperty("effect",effect); row.addProperty("no_gravity",noGravity);
            row.addProperty("bits",Long.toString(Double.doubleToRawLongBits(player.effective())));
            rows.add(row);
          }
    Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
  }
}
