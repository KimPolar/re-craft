import java.io.*;
import java.lang.reflect.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.material.*;

public class StairShapeOracle {
    static final Direction[] HORIZONTAL = {Direction.NORTH, Direction.SOUTH, Direction.WEST, Direction.EAST};
    static class World extends ServerLevel {
        BlockState[] neighbors;
        int scheduled, delay;
        World() { super(null, null, null, null, null, null, false, 0L, null, false); }
        @Override public BlockState getBlockState(BlockPos p) {
            for (int i=0; i<4; i++) if (p.equals(BlockPos.ZERO.relative(HORIZONTAL[i]))) return neighbors[i];
            return Blocks.AIR.defaultBlockState();
        }
        @Override public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
        @Override public void scheduleTick(BlockPos p, Fluid fluid, int d) { scheduled++; delay=d; }
    }
    static BlockState stair(int facing, int half, int shape, boolean wet, boolean stone) {
        return (stone ? Blocks.STONE_STAIRS : Blocks.OAK_STAIRS).defaultBlockState()
            .setValue(StairBlock.FACING, HORIZONTAL[facing])
            .setValue(StairBlock.HALF, half == 0 ? Half.BOTTOM : Half.TOP)
            .setValue(StairBlock.SHAPE, StairsShape.values()[shape])
            .setValue(StairBlock.WATERLOGGED, wet);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        World world=(World)((sun.misc.Unsafe)f.get(null)).allocateInstance(World.class);
        world.neighbors = new BlockState[4];
        Method shape=StairBlock.class.getDeclaredMethod("getStairsShape",BlockState.class,BlockGetter.class,BlockPos.class);
        shape.setAccessible(true);
        Method update=StairBlock.class.getDeclaredMethod("updateShape",BlockState.class,LevelReader.class,ScheduledTickAccess.class,BlockPos.class,Direction.class,BlockPos.class,BlockState.class,RandomSource.class);
        update.setAccessible(true);
        try (var out=new DataOutputStream(Files.newOutputStream(Path.of(args[0])))) {
            for (int facing=0; facing<4; facing++) for (int half=0; half<2; half++) for (int code=0; code<6561; code++) {
                int remaining=code;
                for (int d=0; d<4; d++) {
                    int n=remaining%9; remaining/=9;
                    world.neighbors[d]=n==0 ? Blocks.AIR.defaultBlockState() : stair((n-1)/2,(n-1)%2,(code+d)%5,((code+d)&1)!=0,(d&1)!=0);
                }
                BlockState state=stair(facing,half,code%5,(code&1)!=0,false);
                out.writeByte(((StairsShape)shape.invoke(null,state,world,BlockPos.ZERO)).ordinal());
            }
            for (int wet=0; wet<2; wet++) for (Direction dir: Direction.values()) {
                for (int i=0; i<4; i++) world.neighbors[i]=Blocks.AIR.defaultBlockState();
                world.neighbors[0]=stair(2,0,0,false,true);
                BlockState original=stair(0,0,2,wet!=0,false);
                world.scheduled=0; world.delay=0;
                BlockState updated=(BlockState)update.invoke(Blocks.OAK_STAIRS,original,world,world,BlockPos.ZERO,dir,BlockPos.ZERO.relative(dir),Blocks.AIR.defaultBlockState(),RandomSource.create(0));
                out.writeByte(updated.getValue(StairBlock.SHAPE).ordinal());
                out.writeByte(world.scheduled); out.writeByte(world.delay);
            }
        }
    }
}
