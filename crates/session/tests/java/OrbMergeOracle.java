import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.phys.AABB;

public class OrbMergeOracle {
  static class TestLevel extends ClientLevel {
    TestLevel() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public int getNextEntityId() { return 1; }
    @Override public boolean noCollision(AABB box) { return true; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var level=(TestLevel)((sun.misc.Unsafe)f.get(null)).allocateInstance(TestLevel.class);
    var can=ExperienceOrb.class.getDeclaredMethod("canMerge", ExperienceOrb.class); can.setAccessible(true);
    var merge=ExperienceOrb.class.getDeclaredMethod("merge", ExperienceOrb.class); merge.setAccessible(true);
    var age=ExperienceOrb.class.getDeclaredField("age"); age.setAccessible(true);
    var count=ExperienceOrb.class.getDeclaredField("count"); count.setAccessible(true);
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var results=new JsonArray();
    for (var item : cases) {
      var c=item.getAsJsonObject(); var p=c.getAsJsonArray("position"); var q=c.getAsJsonArray("other_position");
      var a=new ExperienceOrb(level,p.get(0).getAsDouble(),p.get(1).getAsDouble(),p.get(2).getAsDouble(),c.get("value").getAsInt());
      var b=new ExperienceOrb(level,q.get(0).getAsDouble(),q.get(1).getAsDouble(),q.get(2).getAsDouble(),c.get("other_value").getAsInt());
      a.setId(c.get("id").getAsInt()); b.setId(c.get("other_id").getAsInt());
      age.setInt(a,c.get("age").getAsInt()); age.setInt(b,c.get("other_age").getAsInt());
      count.setInt(a,c.get("count").getAsInt()); count.setInt(b,c.get("other_count").getAsInt());
      boolean ok=(boolean)can.invoke(a,b) && a.getBoundingBox().inflate(0.5).intersects(b.getBoundingBox());
      if(ok) merge.invoke(a,b);
      var row=new JsonObject(); row.addProperty("merged",ok); row.addProperty("count",count.getInt(a));
      row.addProperty("age",age.getInt(a)); row.addProperty("removed",b.isRemoved()); results.add(row);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(results));
  }
}
