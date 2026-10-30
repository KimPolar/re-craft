import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.level.block.Blocks;

public class FallingFallDistanceOracle {
  static class Falling extends FallingBlockEntity {
    Falling() { super(null, null); }
    @Override public boolean isInWater() { return false; }
    void check(double y, boolean ground) { super.checkFallDamage(y, ground, Blocks.STONE.defaultBlockState(), null); }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null);
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var result=new JsonArray();
    for(var row:cases) {
      var c=row.getAsJsonObject();
      var entity=(Falling)unsafe.allocateInstance(Falling.class);
      entity.fallDistance=c.get("distance").getAsDouble();
      entity.check(c.get("y").getAsDouble(), c.get("ground").getAsBoolean());
      result.add(entity.fallDistance);
    }
    Files.writeString(Path.of(args[1]), new Gson().toJson(result));
  }
}
