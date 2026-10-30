import java.nio.file.*;
import java.util.List;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.RedstoneLampBlock;
import net.minecraft.world.level.block.state.BlockState;

public class RedstoneLampOracle {
    static class World extends ServerLevel {
        boolean power, result;
        int delay, flags;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public boolean hasNeighborSignal(BlockPos pos) { return power; }
        @Override public void scheduleTick(BlockPos pos, Block block, int delay) { this.delay = delay; }
        @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int limit) {
            result = state.getValue(RedstoneLampBlock.LIT); this.flags = flags; return true;
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world = (World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
        var rows = new JsonArray();
        for (var tick : new boolean[]{false,true}) for (var lit : new boolean[]{false,true}) for (var power : new boolean[]{false,true}) {
            world.power = power; world.result = lit; world.delay = -1; world.flags = -1;
            var state = Blocks.REDSTONE_LAMP.defaultBlockState().setValue(RedstoneLampBlock.LIT, lit);
            if (tick) state.tick(world, BlockPos.ZERO, RandomSource.create(1));
            else state.handleNeighborChanged(world, BlockPos.ZERO, Blocks.STONE, null, false);
            var row = new JsonArray(); row.add(tick); row.add(lit); row.add(power);
            row.add(world.result); row.add(world.delay); row.add(world.flags); rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
