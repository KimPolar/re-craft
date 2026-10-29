import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;

public class OrbFluidOracle {
  static class TestLevel extends ClientLevel {
    TestLevel() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public int getNextEntityId() { return 1; }
    @Override public boolean noCollision(net.minecraft.world.phys.AABB box) { return true; }
  }
  static JsonArray vec(Vec3 v) { var out=new JsonArray(); out.add(v.x); out.add(v.y); out.add(v.z); return out; }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var level=(TestLevel)((sun.misc.Unsafe)f.get(null)).allocateInstance(TestLevel.class);
    var orb=new ExperienceOrb(level,0,0,0,7);
    var water=ExperienceOrb.class.getDeclaredMethod("setUnderwaterMovement"); water.setAccessible(true);
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var result=new JsonArray();
    for (var item : cases) {
      var c=item.getAsJsonObject(); var v=c.getAsJsonArray("velocity");
      orb.setDeltaMovement(v.get(0).getAsDouble(),v.get(1).getAsDouble(),v.get(2).getAsDouble());
      water.invoke(orb);
      var random=new LegacyRandomSource(c.get("seed").getAsLong());
      // ExperienceOrb.tick uses these float operations for the lava impulse.
      var lava=new Vec3((random.nextFloat()-random.nextFloat())*0.2f,0.2f,
                        (random.nextFloat()-random.nextFloat())*0.2f);
      var row=new JsonObject(); row.add("water",vec(orb.getDeltaMovement())); row.add("lava",vec(lava));
      row.addProperty("next",random.nextInt()); row.addProperty("eye",(double)orb.getEyeHeight()); result.add(row);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(result));
  }
}
