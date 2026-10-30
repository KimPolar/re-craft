import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SnowLayerBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.Vec3;

public class SnowPlacementOracle {
  static class World extends ClientLevel {
    BlockState clicked, adjacent; Direction face;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) {
      if (pos.equals(BlockPos.ZERO)) return clicked;
      if (pos.equals(BlockPos.ZERO.relative(face))) return adjacent;
      return Blocks.STONE.defaultBlockState();
    }
  }
  static class Context extends BlockPlaceContext {
    Context(World world, ItemStack stack, BlockHitResult hit) { super(world,null,InteractionHand.MAIN_HAND,stack,hit); }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    Items.SNOW.builtInRegistryHolder().bindComponents(net.minecraft.core.component.DataComponentMap.builder().set(net.minecraft.core.component.DataComponents.MAX_STACK_SIZE,64).build());
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null); var world=(World)unsafe.allocateInstance(World.class);
    var result=new JsonArray();
    for (int clicked=0; clicked<=8; clicked++) for (Direction face:Direction.values()) for (int adjacent:new int[]{0,1,2,7,8}) {
      world.clicked = clicked==0 ? Blocks.STONE.defaultBlockState() : Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS,clicked);
      world.adjacent = adjacent==0 ? Blocks.AIR.defaultBlockState() : Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS,adjacent);
      world.face=face;
      var context=new Context(world,new ItemStack(Items.SNOW),new BlockHitResult(Vec3.atCenterOf(BlockPos.ZERO),face,BlockPos.ZERO,false));
      var target=context.getClickedPos();
      var state=Blocks.SNOW.getStateForPlacement(context);
      boolean place=context.canPlace() && state!=null && state.canSurvive(world,target);
      var row=new JsonObject(); row.addProperty("clicked",clicked); row.addProperty("adjacent",adjacent); row.addProperty("face",face.getName());
      row.addProperty("place",place); row.addProperty("x",target.getX()); row.addProperty("y",target.getY()); row.addProperty("z",target.getZ());
      if(place) row.addProperty("layers",state.getValue(SnowLayerBlock.LAYERS));
      result.add(row);
    }
    Files.writeString(Path.of(args[0]),result.toString());
  }
}
