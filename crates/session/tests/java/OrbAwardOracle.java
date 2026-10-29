import java.nio.file.*;
import java.util.*;
import java.util.function.Predicate;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.level.entity.EntityTypeTest;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.util.RandomSource;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;

public class OrbAwardOracle {
  static class TestClient extends ClientLevel {
    TestClient() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public int getNextEntityId() { return 1; }
    @Override public boolean noCollision(AABB box) { return true; }
  }
  static class TestServer extends ServerLevel {
    RandomSource rng; List<ExperienceOrb> candidates;
    TestServer() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
    @Override public RandomSource getRandom() { return rng; }
    @Override public <T extends Entity> List<T> getEntities(EntityTypeTest<Entity,T> type, AABB box, Predicate<? super T> filter) {
      List<T> result=new ArrayList<>();
      for (var entity : candidates) {
        T cast=type.tryCast(entity);
        if (cast!=null && box.intersects(entity.getBoundingBox()) && filter.test(cast)) result.add(cast);
      }
      return result;
    }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)f.get(null);
    var level=(TestClient)unsafe.allocateInstance(TestClient.class);
    var server=(TestServer)unsafe.allocateInstance(TestServer.class);
    var merge=ExperienceOrb.class.getDeclaredMethod("tryMergeToExisting",ServerLevel.class,Vec3.class,int.class); merge.setAccessible(true);
    var age=ExperienceOrb.class.getDeclaredField("age"); age.setAccessible(true);
    var count=ExperienceOrb.class.getDeclaredField("count"); count.setAccessible(true);
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var results=new JsonArray();
    for (var item : cases) {
      var c=item.getAsJsonObject(); var p=c.getAsJsonArray("position");
      var pos=new Vec3(p.get(0).getAsDouble(),p.get(1).getAsDouble(),p.get(2).getAsDouble());
      server.rng=new LegacyRandomSource(c.get("seed").getAsLong()); server.candidates=new ArrayList<>();
      for (var raw : c.getAsJsonArray("orbs")) {
        var b=raw.getAsJsonObject(); var q=b.getAsJsonArray("position");
        var orb=new ExperienceOrb(level,q.get(0).getAsDouble(),q.get(1).getAsDouble(),q.get(2).getAsDouble(),b.get("value").getAsInt());
        orb.setId(b.get("id").getAsInt()); age.setInt(orb,b.get("age").getAsInt()); count.setInt(orb,b.get("count").getAsInt()); server.candidates.add(orb);
      }
      boolean merged=(boolean)merge.invoke(null,server,pos,c.get("value").getAsInt());
      var row=new JsonObject(); row.addProperty("merged",merged); row.addProperty("next",server.rng.nextInt());
      var all=new JsonArray(); for(var orb:server.candidates) { var e=new JsonArray(); e.add(count.getInt(orb)); e.add(age.getInt(orb)); all.add(e); }
      row.add("orbs",all); results.add(row);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(results));
  }
}
