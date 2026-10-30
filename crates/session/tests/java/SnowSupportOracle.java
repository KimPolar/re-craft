import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.TagLoader;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;

public class SnowSupportOracle {
  static class World extends ClientLevel {
    BlockState support;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) { return support; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
      BlockTags.CANNOT_SUPPORT_SNOW_LAYER,List.of(Blocks.STONE.builtInRegistryHolder()),
      BlockTags.SUPPORT_OVERRIDE_SNOW_LAYER,List.of(Blocks.STONE.builtInRegistryHolder(),Blocks.ICE.builtInRegistryHolder())
    ))).apply();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var world=(World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
    var result=new JsonArray();
    for(var block:new net.minecraft.world.level.block.Block[]{Blocks.STONE,Blocks.ICE,Blocks.AIR,Blocks.DIRT,Blocks.FARMLAND,Blocks.OAK_SLAB,Blocks.OAK_LEAVES,Blocks.SNOW})
      for(var state:block.getStateDefinition().getPossibleStates()) {
        world.support=state;
        var row=new JsonObject(); row.addProperty("id",BuiltInRegistries.BLOCK.getKey(block).toString());
        var properties=new JsonObject(); state.getValues().forEach(value -> properties.addProperty(value.property().getName(),value.valueName()));
        row.add("properties",properties); row.addProperty("can_survive",Blocks.SNOW.defaultBlockState().canSurvive(world,BlockPos.ZERO));
        var updated=Blocks.SNOW.defaultBlockState().updateShape(world,null,BlockPos.ZERO,net.minecraft.core.Direction.DOWN,BlockPos.ZERO.below(),state,net.minecraft.util.RandomSource.create(0L));
        row.addProperty("kept_after_update",updated.is(Blocks.SNOW));
        result.add(row);
      }
    Files.writeString(Path.of(args[0]),result.toString());
  }
}
