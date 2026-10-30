import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.context.DirectionalPlaceContext;
import net.minecraft.world.level.block.state.BlockState;

public class FallingReplacementOracle {
  static class World extends ClientLevel {
    BlockState state;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) { return state; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null);
    var world=(World)unsafe.allocateInstance(World.class);
    var result=new JsonArray();
    for (int i=1; i<args.length; i++) {
      var id=Identifier.parse(args[i]);
      var block=BuiltInRegistries.BLOCK.getValue(id);
      if (block == null || !BuiltInRegistries.BLOCK.getKey(block).equals(id)) throw new AssertionError("missing block " + id);
      for (var state:block.getStateDefinition().getPossibleStates()) {
        world.state=state;
        var context=new DirectionalPlaceContext(world,BlockPos.ZERO,Direction.DOWN,ItemStack.EMPTY,Direction.UP);
        var row=new JsonObject(); row.addProperty("id",args[i]); row.addProperty("replace",state.canBeReplaced(context));
        var properties=new JsonObject(); state.getValues().forEach(value -> properties.addProperty(value.property().getName(),value.valueName()));
        row.add("properties",properties); result.add(row);
      }
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(result));
  }
}
