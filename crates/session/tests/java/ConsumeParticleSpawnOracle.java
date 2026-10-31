import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.particles.ParticleOptions;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.*;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.GameType;
import net.minecraft.world.phys.Vec3;

public class ConsumeParticleSpawnOracle {
  static class World extends ClientLevel {
    JsonArray particles;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public void addParticle(ParticleOptions option,double x,double y,double z,double dx,double dy,double dz) {
      var row=new JsonArray(); for(double value:new double[]{x,y,z,dx,dy,dz}) row.add(Double.doubleToRawLongBits(value)); particles.add(row);
    }
  }
  static class TestPlayer extends Player {
    World world; float yaw,pitch;
    TestPlayer() { super(null,null); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public Level level() { return world; }
    @Override public float getYRot() { return yaw; }
    @Override public float getXRot() { return pitch; }
    @Override public double getEyeY() { return 65.62; }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var lookup=net.minecraft.data.registries.VanillaRegistries.createWorldLookup();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.world=world;
    var position=Entity.class.getDeclaredField("position"); position.setAccessible(true); position.set(player,new Vec3(12.25,64,-30.75));
    var random=Entity.class.getDeclaredField("random"); long randomOffset=unsafe.objectFieldOffset(random);
    var out=new JsonArray();
    for(int seed=0;seed<8;seed++) for(float yaw:new float[]{-180f,-37.5f,0f,90f,179.5f}) for(float pitch:new float[]{-90f,-25.5f,0f,40f,90f}) {
      player.yaw=yaw; player.pitch=pitch; unsafe.putObject(player,randomOffset,RandomSource.create(seed)); world.particles=new JsonArray();
      player.spawnItemParticles(new ItemStack(Items.APPLE),5);
      var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("yaw",yaw); row.addProperty("pitch",pitch);
      row.add("particles",world.particles); row.addProperty("next_bits",Float.floatToRawIntBits(player.getRandom().nextFloat())); out.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}
