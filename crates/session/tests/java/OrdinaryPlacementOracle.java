import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SnowLayerBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.Vec3;

public class OrdinaryPlacementOracle {
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
  static BlockState state(String id) {
    if (id.startsWith("snow_")) return Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS,Integer.parseInt(id.substring(5)));
    return BuiltInRegistries.BLOCK.getValue(Identifier.withDefaultNamespace(id)).defaultBlockState();
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var worldLookup=net.minecraft.data.registries.VanillaRegistries.createWorldLookup();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(worldLookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var world=(World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
    String[] items={"stone","dirt","oak_log"};
    String[] blocks={"stone","air","snow_1","snow_2","snow_8","water","lava","short_grass","fern","dead_bush","fire","soul_fire", "torch", "wall_torch", "soul_torch", "soul_wall_torch", "oak_sapling", "spruce_sapling", "cherry_sapling", "dandelion", "poppy"};
    var result=new JsonArray();
    for(String item:items) for(String clicked:blocks) for(String adjacent:blocks) for(Direction face:Direction.values()) {
      world.clicked=state(clicked); world.adjacent=state(adjacent); world.face=face;
      var stack=new ItemStack(BuiltInRegistries.ITEM.getValue(Identifier.withDefaultNamespace(item)),1);
      var context=new Context(world,stack,new BlockHitResult(Vec3.atCenterOf(BlockPos.ZERO),face,BlockPos.ZERO,false));
      var pos=context.getClickedPos();
      var row=new JsonObject(); row.addProperty("item",item); row.addProperty("clicked",clicked); row.addProperty("adjacent",adjacent);
      row.addProperty("face",face.getName()); row.addProperty("can_place",context.canPlace());
      row.addProperty("x",pos.getX()); row.addProperty("y",pos.getY()); row.addProperty("z",pos.getZ()); result.add(row);
    }
    Files.writeString(Path.of(args[0]),result.toString());
  }
}
