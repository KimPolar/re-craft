import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.TagLoader;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SnowyBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.Vec3;
import net.minecraft.util.RandomSource;

public class SnowyBlockOracle {
  static class World extends ClientLevel {
    BlockState above;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) {
      if (pos.equals(BlockPos.ZERO.above())) return above;
      if (pos.equals(BlockPos.ZERO.below())) return Blocks.STONE.defaultBlockState();
      return Blocks.AIR.defaultBlockState();
    }
  }
  static class Context extends BlockPlaceContext {
    Context(World world, BlockHitResult hit) { super(world,null,InteractionHand.MAIN_HAND,ItemStack.EMPTY,hit); }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
      BlockTags.SNOW,List.of(Blocks.STONE.builtInRegistryHolder(),Blocks.SNOW.builtInRegistryHolder(),Blocks.SNOW_BLOCK.builtInRegistryHolder())
    ))).apply();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var world=(World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
    var rows=new JsonArray(); var names=new JsonArray();
    for (Block block:BuiltInRegistries.BLOCK) {
      if (!(block instanceof SnowyBlock)) continue;
      String name=BuiltInRegistries.BLOCK.getKey(block).toString(); names.add(name);
      for (Block upper:new Block[]{Blocks.AIR,Blocks.STONE,Blocks.DIRT,Blocks.SNOW,Blocks.SNOW_BLOCK,Blocks.POWDER_SNOW}) {
        world.above=upper.defaultBlockState();
        var hit=new BlockHitResult(Vec3.atCenterOf(BlockPos.ZERO.below()),Direction.UP,BlockPos.ZERO.below(),false);
        var context=new Context(world,hit);
        boolean placed=block.getStateForPlacement(context).getValue(SnowyBlock.SNOWY);
        for (boolean initial:new boolean[]{false,true}) for (Direction dir:Direction.values()) {
          var state=block.defaultBlockState().setValue(SnowyBlock.SNOWY,initial);
          var updated=state.updateShape(world,null,BlockPos.ZERO,dir,BlockPos.ZERO.relative(dir),world.above,RandomSource.create(0L));
          var row=new JsonObject(); row.addProperty("block",name);
          row.addProperty("above",BuiltInRegistries.BLOCK.getKey(upper).toString());
          row.addProperty("direction",dir.getName()); row.addProperty("initial",initial);
          row.addProperty("updated",updated.getValue(SnowyBlock.SNOWY)); row.addProperty("placed",placed);
          rows.add(row);
        }
      }
    }
    var result=new JsonObject(); result.add("blocks",names); result.add("rows",rows);
    Files.writeString(Path.of(args[0]),result.toString());
  }
}
