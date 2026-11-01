import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.BlockBehaviour;

public class BannerSupportOracle {
  static class World extends ClientLevel {
    BlockState support;
    World(){super(null,null,null,null,0,0,null,false,0L,0);}
    @Override public BlockState getBlockState(BlockPos pos){return support;}
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion();net.minecraft.server.Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);var world=(World)unsafe.allocateInstance(World.class);
    var field=BlockBehaviour.class.getDeclaredField("UPDATE_SHAPE_ORDER");field.setAccessible(true);var order=(Direction[])field.get(null);var json=new JsonObject();var dirs=new JsonArray();for(var d:order)dirs.add(d.getName());json.add("order",dirs);
    var rows=new JsonArray();
    for(String face:new String[]{"standing","north","south","west","east"}) for(Direction changed:Direction.values()) for(boolean solid:new boolean[]{false,true}) {
      BlockState banner=face.equals("standing")?Blocks.BANNER.white().defaultBlockState():Blocks.WALL_BANNER.white().defaultBlockState().setValue(WallBannerBlock.FACING,Direction.byName(face));
      world.support=solid?Blocks.STONE.defaultBlockState():Blocks.AIR.defaultBlockState();
      var result=banner.updateShape(world,world,BlockPos.ZERO,changed,BlockPos.ZERO.relative(changed),world.support,RandomSource.create(1));
      var row=new JsonObject();row.addProperty("face",face);row.addProperty("changed",changed.getName());row.addProperty("solid",solid);row.addProperty("removed",result.isAir());rows.add(row);
    }
    json.add("rows",rows);Files.writeString(Path.of(args[0]),json.toString());
  }
}
