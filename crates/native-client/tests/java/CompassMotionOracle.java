import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.client.renderer.item.properties.numeric.CompassAngleState;
import net.minecraft.core.BlockPos;
import net.minecraft.world.TickRateManager;
import net.minecraft.world.entity.ItemOwner;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.Level;
import net.minecraft.world.phys.Vec3;

public class CompassMotionOracle {
  static class World extends ClientLevel {
    TickRateManager rate;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public TickRateManager tickRateManager() { return rate; }
  }
  static class Local extends Player {
    World world; Vec3 point; float yaw;
    Local() { super(null,null); }
    @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
    @Override public boolean isLocalPlayer() { return true; }
    @Override public Level level() { return world; }
    @Override public Vec3 position() { return point; }
    @Override public float getVisualRotationYInDegrees() { return yaw; }
  }
  static class Dropped extends net.minecraft.world.entity.item.ItemEntity {
    Vec3 point; int lifetime;
    Dropped() { super((net.minecraft.world.entity.EntityType<? extends net.minecraft.world.entity.item.ItemEntity>)null, null); }
    @Override public Vec3 position() { return point; }
    @Override public int getAge() { return lifetime; }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); world.rate=new TickRateManager();
    var player=(Local)unsafe.allocateInstance(Local.class); player.world=world;
    var state=new CompassAngleState(true,CompassAngleState.CompassTarget.LODESTONE);
    var method=CompassAngleState.class.getDeclaredMethod("getRotationTowardsCompassTarget",ItemOwner.class,long.class,BlockPos.class); method.setAccessible(true);
    var out=new JsonArray();
    for(int n=0;n<240;n++) {
      long tick=n/2; player.yaw=(float)((n*47)%720-360);
      player.point=new Vec3(16000000.25 + (n%7)*0.13,65.5 + (n%3),-15999999.75-(n%11)*0.17);
      var target=new BlockPos(16000000 + (n%19)-9,64,-16000000 + (n%23)-11);
      float result=(float)method.invoke(state,player,tick,target);
      var row=new JsonObject(); row.addProperty("tick",tick); row.addProperty("yaw",player.yaw);
      row.addProperty("x",player.point.x); row.addProperty("y",player.point.y); row.addProperty("z",player.point.z);
      row.addProperty("tx",target.getX()); row.addProperty("ty",target.getY()); row.addProperty("tz",target.getZ());
      row.addProperty("bits",Float.floatToRawIntBits(result)); out.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
    if (args.length > 1) {
      var randomState=new CompassAngleState(true,CompassAngleState.CompassTarget.LODESTONE);
      var randomField=CompassAngleState.class.getDeclaredField("random"); randomField.setAccessible(true);
      unsafe.putObject(randomState,unsafe.objectFieldOffset(randomField),net.minecraft.util.RandomSource.create(1234567L));
      var spin=CompassAngleState.class.getDeclaredMethod("getRandomlySpinningRotation",int.class,long.class); spin.setAccessible(true);
      var rotations=new JsonArray(); int[] seeds={0,1,10,12345,-1,Integer.MIN_VALUE,Integer.MAX_VALUE};
      for(int n=0;n<245;n++) {
        int seed=seeds[n%seeds.length]; long tick=n/3;
        float value=(float)spin.invoke(randomState,seed,tick);
        var row=new JsonObject(); row.addProperty("tick",tick); row.addProperty("seed",seed); row.addProperty("bits",Float.floatToRawIntBits(value)); rotations.add(row);
      }
      Files.writeString(Path.of(args[1]),new Gson().toJson(rotations));
    }
    if (args.length > 2) {
      var dropped=(Dropped)unsafe.allocateInstance(Dropped.class);
      var bob=net.minecraft.world.entity.item.ItemEntity.class.getDeclaredField("bobOffs"); bob.setAccessible(true); long bobOffset=unsafe.objectFieldOffset(bob);
      var samples=new JsonArray(); int[] ages={-32768,-100,0,1,5999}; float[] phases={0.0f,1.25f,5.2f};
      for(int n=0;n<240;n++) {
        dropped.lifetime=ages[n%ages.length]; float phase=phases[n%phases.length]; unsafe.putFloat(dropped,bobOffset,phase);
        dropped.point=new Vec3(16000000.25+(n%7)*0.13,65.5+(n%3),-15999999.75-(n%11)*0.17);
        var target=new BlockPos(16000000+(n%19)-9,64,-16000000+(n%23)-11);
        float result=(float)method.invoke(state,dropped,(long)(n/2),target);
        var row=new JsonObject(); row.addProperty("age",dropped.lifetime); row.addProperty("bob",phase);
        row.addProperty("x",dropped.point.x); row.addProperty("y",dropped.point.y); row.addProperty("z",dropped.point.z);
        row.addProperty("tx",target.getX()); row.addProperty("ty",target.getY()); row.addProperty("tz",target.getZ()); row.addProperty("bits",Float.floatToRawIntBits(result)); samples.add(row);
      }
      Files.writeString(Path.of(args[2]),new Gson().toJson(samples));
    }
  }
}
