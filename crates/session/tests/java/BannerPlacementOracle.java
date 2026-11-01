import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.core.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.world.item.*;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.shapes.CollisionContext;

public class BannerPlacementOracle {
  static class World extends ClientLevel {
    Set<Direction> supports;
    World(){super(null,null,null,null,0,0,null,false,0L,0);}
    @Override public BlockState getBlockState(BlockPos p) { for(var direction:Direction.values()) if(p.equals(BlockPos.ZERO.relative(direction))) return supports.contains(direction)?Blocks.STONE.defaultBlockState():Blocks.AIR.defaultBlockState(); return Blocks.AIR.defaultBlockState(); }
    @Override public boolean isUnobstructed(BlockState s,BlockPos p,CollisionContext c){return true;}
  }
  static class Context extends BlockPlaceContext {
    Level world; Direction[] directions; float rotation;
    Context(){super(null,null,null,null,null);}
    @Override public Direction[] getNearestLookingDirections(){return directions;}
    @Override public float getRotation(){return rotation;}
    @Override public Level getLevel(){return world;}
    @Override public BlockPos getClickedPos(){return BlockPos.ZERO;}
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var context=(Context)unsafe.allocateInstance(Context.class);context.world=world;
    var method=StandingAndWallBlockItem.class.getDeclaredMethod("getPlacementState",BlockPlaceContext.class);method.setAccessible(true);
    var output=new JsonArray();
    for(var entry:JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray()) {
      var input=entry.getAsJsonObject(); context.rotation=input.get("rotation").getAsFloat();
      context.directions=new Direction[6];int i=0;for(var d:input.getAsJsonArray("directions"))context.directions[i++]=Direction.valueOf(d.getAsString());
      world.supports=EnumSet.noneOf(Direction.class);for(var d:input.getAsJsonArray("supports"))world.supports.add(Direction.valueOf(d.getAsString()));
      var state=(BlockState)method.invoke(Items.BANNER.white(),context);
      if(state==null){output.add(JsonNull.INSTANCE);continue;}
      var row=new JsonObject();row.addProperty("id",net.minecraft.core.registries.BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
      var props=new JsonObject();state.getValues().forEach(v->props.addProperty(v.property().getName(),v.valueName()));row.add("properties",props);output.add(row);
    }
    Files.writeString(Path.of(args[1]),output.toString());
  }
}
