import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.MoverType;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.ItemLike;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.phys.Vec3;

public class FallingExpiryOracle {
  static class World extends ServerLevel {
    GameRules rules;
    World() { super(null,null,null,null,null,null,false,0L,null,false); }
    @Override public GameRules getGameRules() { return rules; }
    @Override public int getMinY() { return -64; }
    @Override public int getHeight() { return 384; }
    @Override public BlockState getBlockState(BlockPos pos) { return Blocks.AIR.defaultBlockState(); }
  }
  static class Falling extends FallingBlockEntity {
    World world; BlockPos pos; Vec3 velocity; boolean removed; boolean dropped; boolean grounded;
    Falling() { super(null,null); }
    @Override public Level level() { return world; }
    @Override public BlockPos blockPosition() { return pos; }
    @Override public boolean isAlive() { return true; }
    @Override public boolean onGround() { return grounded; }
    @Override public boolean isNoGravity() { return true; }
    @Override public Vec3 getDeltaMovement() { return velocity; }
    @Override public void setDeltaMovement(Vec3 v) { velocity=v; }
    @Override public void move(MoverType type, Vec3 v) { }
    @Override protected void applyEffectsFromBlocks() { }
    @Override protected void handlePortal() { }
    @Override public void setRequiresPrecisePosition(boolean v) { }
    @Override public void remove(Entity.RemovalReason reason) { removed=true; }
    @Override public ItemEntity spawnAtLocation(ServerLevel world, ItemLike item) { dropped=true; return null; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    Field block=FallingBlockEntity.class.getDeclaredField("blockState"); block.setAccessible(true);
    var world=(World)unsafe.allocateInstance(World.class); world.rules=new GameRules(FeatureFlags.DEFAULT_FLAGS);
    var output=new JsonArray();
    for (int age : new int[]{0,99,100,599,600}) for (int y : new int[]{-65,-64,-63,0,319,320}) for (boolean drops : new boolean[]{false,true}) {
      world.rules.set(GameRules.ENTITY_DROPS,drops,null);
      var e=(Falling)unsafe.allocateInstance(Falling.class); e.world=world; e.pos=new BlockPos(0,y,0); e.velocity=Vec3.ZERO;
      e.time=age; e.dropItem=true; block.set(e,Blocks.SAND.defaultBlockState()); e.tick();
      var row=new JsonObject(); row.addProperty("age",e.time); row.addProperty("y",y); row.addProperty("drops",drops);
      row.addProperty("removed",e.removed); row.addProperty("dropped",e.dropped); output.add(row);
    }
    for (boolean drops : new boolean[]{false,true}) {
      world.rules.set(GameRules.ENTITY_DROPS,drops,null);
      var e=(Falling)unsafe.allocateInstance(Falling.class); e.world=world; e.pos=new BlockPos(0,1,0); e.velocity=Vec3.ZERO;
      e.grounded=true; e.dropItem=true; block.set(e,Blocks.SAND.defaultBlockState()); e.tick();
      var row=new JsonObject(); row.addProperty("landing",true); row.addProperty("drops",drops);
      row.addProperty("removed",e.removed); row.addProperty("dropped",e.dropped); output.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(output));
  }
}
