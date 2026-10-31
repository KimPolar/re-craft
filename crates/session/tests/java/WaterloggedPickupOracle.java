import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.world.item.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.*;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.phys.*;
import net.minecraft.world.phys.shapes.CollisionContext;

public class WaterloggedPickupOracle {
  static final BlockPos TARGET=BlockPos.ZERO;
  static class World extends ClientLevel {
    Map<BlockPos,BlockState> blocks;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState()); }
    @Override public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int depth) { blocks.put(pos,state); return true; }
    @Override public boolean isClientSide() { return false; }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
    for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var world=(World)unsafe.allocateInstance(World.class); var output=new JsonArray();
    for(String shape:new String[]{"bottom","top","stairs"}) for(float height:new float[]{0.25f,0.49f,0.51f,0.75f,0.89f,0.95f}) {
      BlockState block=shape.equals("stairs")?Blocks.OAK_STAIRS.defaultBlockState().setValue(StairBlock.WATERLOGGED,true):Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,shape.equals("top")?SlabType.TOP:SlabType.BOTTOM).setValue(SlabBlock.WATERLOGGED,true);
      world.blocks=new HashMap<>(); world.blocks.put(TARGET,block); world.blocks.put(TARGET.north(),Blocks.STONE.defaultBlockState());
      Vec3 from=new Vec3(0.5,(double)height,2.5), to=new Vec3(0.5,(double)height,-2.5);
      var hit=world.clip(new ClipContext(from,to,ClipContext.Block.OUTLINE,ClipContext.Fluid.SOURCE_ONLY,CollisionContext.empty()));
      var row=new JsonObject(); row.addProperty("shape",shape); row.addProperty("height",height); row.addProperty("z",hit.getBlockPos().getZ()); row.addProperty("face",hit.getDirection().getName()); row.addProperty("hit_z",hit.getLocation().z);
      if(hit.getBlockPos().equals(TARGET)) {
        var item=((BucketPickup)block.getBlock()).pickupBlock(null,world,TARGET,block);
        row.addProperty("item",item.is(Items.WATER_BUCKET)); row.addProperty("wet",world.getBlockState(TARGET).getValue(BlockStateProperties.WATERLOGGED));
      }
      output.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(output));
  }
}
