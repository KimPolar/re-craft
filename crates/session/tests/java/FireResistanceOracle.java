import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.entity.player.Abilities;

public class FireResistanceOracle {
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null);
    var player=(DamageCooldownOracle.TestPlayer)unsafe.allocateInstance(DamageCooldownOracle.TestPlayer.class);
    var abilities=Player.class.getDeclaredField("abilities");
    unsafe.putObject(player,unsafe.objectFieldOffset(abilities),new Abilities());
    var source=new DamageCooldownOracle.Source(); var out=new JsonArray();
    for(boolean fire:new boolean[]{false,true}) for(boolean resistant:new boolean[]{false,true}) for(int timer:new int[]{0,5,15}) {
      player.fireResistance=resistant; player.damageCooldownTime=timer; player.setLast(2f); player.called=false; player.captured=0;
      source.fire=fire; source.bypass=false;
      try { player.hurtServer(null,source,4f); } catch(DamageCooldownOracle.Captured expected) { }
      var row=new JsonObject(); row.addProperty("fire",fire); row.addProperty("resistant",resistant); row.addProperty("timer",timer); row.addProperty("accepted",player.called);
      if(player.called) row.addProperty("damage",player.captured);
      out.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}
