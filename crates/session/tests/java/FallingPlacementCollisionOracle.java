import java.nio.file.*;
import java.util.*;
import java.util.function.Predicate;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.EntityGetter;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SnowLayerBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.entity.EntityTypeTest;
import net.minecraft.world.phys.AABB;

public class FallingPlacementCollisionOracle {
  static class World extends ClientLevel {
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public int getNextEntityId() { return 1; }
  }
  static class Getter implements EntityGetter {
    Entity entity;
    @Override public List<Entity> getEntities(Entity except, AABB bounds, Predicate<? super Entity> predicate) {
      return entity != except && entity.getBoundingBox().intersects(bounds) && predicate.test(entity) ? List.of(entity) : List.of();
    }
    @Override public <T extends Entity> List<T> getEntities(EntityTypeTest<Entity,T> test, AABB bounds, Predicate<? super T> predicate) { return List.of(); }
    @Override public List<? extends Player> players() { return List.of(); }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var world=(World)((sun.misc.Unsafe)f.get(null)).allocateInstance(World.class);
    var constructor=FallingBlockEntity.class.getDeclaredConstructor(Level.class,double.class,double.class,double.class,BlockState.class);
    constructor.setAccessible(true);
    var created=constructor.newInstance(world,0.5,0.0,0.5,Blocks.SAND.defaultBlockState());
    var loaded=new FallingBlockEntity(EntityTypes.FALLING_BLOCK,world);
    if (!created.blocksBuilding || loaded.blocksBuilding) throw new AssertionError("falling block construction flags");
    var getter=new Getter(); var output=new JsonArray();
    double half=(double)(0.98f)/2.0;
    double[] xs={-half-1e-6,-half,-half+1e-8,-half+5e-8,-half+1e-7,-half+2e-7,0.5,1+half-2e-7,1+half-1e-7,1+half-5e-8,1+half,1+half+1e-6};
    double[] ys={-0.98f,-0.98f+1e-8,-0.98f+2e-7,0.0,0.5,0.875,1-1e-8,1-2e-7,1.0};
    for(int block=0;block<=9;block++) {
      var state=block==0 ? Blocks.STONE.defaultBlockState() : block==9 ? Blocks.CHEST.defaultBlockState() : Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS,block);
      var shape=state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO);
      for(boolean fresh:new boolean[]{false,true}) for(double x:xs) for(double y:ys) {
        getter.entity=fresh ? created : loaded; getter.entity.setPos(x,y,0.5);
        var row=new JsonObject(); row.addProperty("block",block); row.addProperty("fresh",fresh);
        row.addProperty("x",x); row.addProperty("y",y); row.addProperty("clear",getter.isUnobstructed(null,shape)); output.add(row);
      }
    }
    Files.writeString(Path.of(args[0]),output.toString());
  }
}
