import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.*;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.material.*;

public class LavaDelayOracle {
  static class World extends ServerLevel {
    EnvironmentAttributeSystem attrs; RandomSource rng; FluidState above;
    World() { super(null,null,null,null,null,null,false,0L,null,false); }
    @Override public EnvironmentAttributeSystem environmentAttributes() { return attrs; }
    @Override public RandomSource getRandom() { return rng; }
    @Override public FluidState getFluidState(BlockPos pos) { return pos.equals(BlockPos.ZERO.above()) ? above : Fluids.EMPTY.defaultFluidState(); }
  }
  static FluidState lava(int level) {
    return level < 0 ? Fluids.EMPTY.defaultFluidState() : Blocks.LAVA.defaultBlockState().setValue(LiquidBlock.LEVEL,level).getFluidState();
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var world=(World)((sun.misc.Unsafe)f.get(null)).allocateInstance(World.class);
    var rows=new JsonArray();
    for(boolean fast:new boolean[]{false,true}) {
      world.attrs=EnvironmentAttributeSystem.builder().addConstantLayer(EnvironmentAttributeMap.builder().set(EnvironmentAttributes.FAST_LAVA,fast).build()).build();
      for(int above=0;above<3;above++) for(int old=-1;old<16;old++) for(int next=-1;next<16;next++) for(int seed=-3;seed<=3;seed++) {
        world.above=above==0 ? Fluids.EMPTY.defaultFluidState() : above==1 ? Fluids.LAVA.defaultFluidState() : Fluids.WATER.defaultFluidState();
        world.rng=RandomSource.create(seed);
        int delay=((LavaFluid)Fluids.FLOWING_LAVA).getSpreadDelay(world,BlockPos.ZERO,lava(old),lava(next));
        var row=new JsonArray(); row.add(fast); row.add(above); row.add(old); row.add(next); row.add(seed); row.add(delay); row.add(world.rng.nextInt()); rows.add(row);
      }
    }
    Files.writeString(Path.of(args[0]),rows.toString());
  }
}
