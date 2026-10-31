import java.nio.file.*;
import java.util.*;
import java.util.function.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.FluidTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.*;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.blockscan.*;
import net.minecraft.world.level.material.Fluids;

public class FarmlandRandomOracle {
    static class World extends ServerLevel {
        BlockPos source;
        BlockState fluidBlock, written;
        boolean rain, maintain;
        int flags;
        World() { super(null,null,null,null,null,null,false,0,List.of(),false); }
        @Override public boolean isRainingAt(BlockPos pos) { return rain; }
        @Override public BlockState getBlockState(BlockPos pos) { return (maintain ? Blocks.WHEAT : Blocks.AIR).defaultBlockState(); }
        @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int depth) { written=state; this.flags=flags; return true; }
        @Override public BlockMatcher findBlocksIn(BlockPos from, BlockPos to) { return new Matcher(this,from,to); }
    }
    static class Matcher extends BlockMatcher {
        World world; BlockPos from,to;
        Matcher(World world, BlockPos from, BlockPos to) { super(world); this.world=world; this.from=from; this.to=to; }
        @Override protected boolean anyMatched(Predicate<BlockState> p) {
            BlockPos s=world.source;
            return s.getX()>=from.getX() && s.getX()<=to.getX() && s.getY()>=from.getY() && s.getY()<=to.getY() && s.getZ()>=from.getZ() && s.getZ()<=to.getZ() && p.test(world.fluidBlock);
        }
        @Override public boolean atLeastMatched(int count) { throw new UnsupportedOperationException(); }
        @Override public void forEach(BiConsumer<BlockPos,BlockState> callback) { throw new UnsupportedOperationException(); }
        @Override public boolean forEachUntil(BlockStateConsumer callback) { throw new UnsupportedOperationException(); }
    }
    static class Farmland extends FarmlandBlock {
        boolean dirt;
        Farmland() { super(Blocks.DIRT,BlockBehaviour.Properties.of()); }
        @Override public void turnToBaseBlock(Entity entity,BlockState state,Level level,BlockPos pos) { dirt=true; }
        void run(BlockState state, World world) { randomTick(state,world,BlockPos.ZERO,RandomSource.create(42)); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
        bind.invoke(Fluids.WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
        bind.invoke(Fluids.FLOWING_WATER.builtInRegistryHolder(),List.of(FluidTags.WATER));
        bind.invoke(Blocks.WHEAT.builtInRegistryHolder(),List.of(BlockTags.MAINTAINS_FARMLAND));
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var farmland=(Farmland)unsafe.allocateInstance(Farmland.class); var rows=new JsonArray();
        int[][] offsets={{-5,0,0},{-4,0,-4},{4,1,4},{5,0,0},{0,-1,1},{0,2,1},{1,0,0}};
        for(int moisture=0;moisture<=7;moisture++) for(var offset:offsets) for(int fluid=0;fluid<4;fluid++)
        for(boolean rain:new boolean[]{false,true}) for(boolean maintain:new boolean[]{false,true}) {
            world.source=new BlockPos(offset[0],offset[1],offset[2]);
            world.fluidBlock=switch(fluid) {
                case 0 -> Blocks.WATER.defaultBlockState();
                case 1 -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,7);
                case 2 -> Blocks.OAK_SLAB.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,true);
                default -> Blocks.LAVA.defaultBlockState();
            };
            world.rain=rain; world.maintain=maintain; world.written=null; world.flags=-1; farmland.dirt=false;
            farmland.run(Blocks.FARMLAND.defaultBlockState().setValue(FarmlandBlock.MOISTURE,moisture),world);
            var row=new JsonArray(); row.add(moisture); row.add(offset[0]); row.add(offset[1]); row.add(offset[2]);
            row.add(fluid); row.add(rain); row.add(maintain);
            row.add(farmland.dirt ? -1 : world.written == null ? moisture : world.written.getValue(FarmlandBlock.MOISTURE));
            row.add(world.flags); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
