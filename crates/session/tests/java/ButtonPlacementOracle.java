import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.item.context.BlockPlaceContext;

public class ButtonPlacementOracle {
  static class World extends ClientLevel {
    BlockState support;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) { return support; }
  }
  static class Context extends BlockPlaceContext {
    Level world; Direction face; Direction horizontal;
    Context() { super(null,null,null,null,null); }
    @Override public Direction[] getNearestLookingDirections() { return new Direction[]{face.getOpposite()}; }
    @Override public Direction getHorizontalDirection() { return horizontal; }
    @Override public Level getLevel() { return world; }
    @Override public BlockPos getClickedPos() { return BlockPos.ZERO; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var delayField=net.minecraft.world.level.block.ButtonBlock.class.getDeclaredField("ticksToStayPressed");
    delayField.setAccessible(true);
    if (delayField.getInt(Blocks.STONE_BUTTON) != 20) throw new AssertionError("unexpected stone button duration");
    var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe = (sun.misc.Unsafe)field.get(null);
    var world = (World)unsafe.allocateInstance(World.class);
    var ctx = (Context)unsafe.allocateInstance(Context.class); ctx.world=world;
    var result = new JsonArray();
    for(var entry: JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray()) {
      var input=entry.getAsJsonObject();
      ctx.face=Direction.valueOf(input.get("face").getAsString());
      ctx.horizontal=Direction.fromYRot(input.get("yaw").getAsDouble());
      world.support=Blocks.STONE.defaultBlockState();
      var placed=Blocks.STONE_BUTTON.getStateForPlacement(ctx);
      if (placed == null) throw new AssertionError("stone support rejected " + ctx.face);
      world.support=Blocks.AIR.defaultBlockState();
      for (var powered: new boolean[]{false,true}) for (var changed : Direction.values()) {
        var before=placed.setValue(net.minecraft.world.level.block.ButtonBlock.POWERED,powered);
        var after=before.updateShape(world,world,BlockPos.ZERO,changed,BlockPos.ZERO.relative(changed),world.support,net.minecraft.util.RandomSource.create(1));
        if ((after.isAir()) != (changed == ctx.face.getOpposite()))
          throw new AssertionError("wrong support update for " + ctx.face + " changed " + changed + " powered=" + powered);
      }
      var properties = new JsonObject();
      placed.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
      result.add(properties);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(result));
  }
}
