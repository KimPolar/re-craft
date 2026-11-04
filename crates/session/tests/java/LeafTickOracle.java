import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;

public class LeafTickOracle {
    static final BlockPos CENTER = new BlockPos(8, 8, 8);
    static Map<BlockPos, BlockState> blocks = new HashMap<>();
    static BlockState result;
    static class FakeLevel extends ServerLevel {
        FakeLevel() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public BlockState getBlockState(BlockPos p) {
            return blocks.getOrDefault(p, Blocks.STONE.defaultBlockState());
        }
        @Override public boolean setBlockAndUpdate(BlockPos p, BlockState state) {
            result = state;
            return true;
        }
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var level = (FakeLevel)unsafe.allocateInstance(FakeLevel.class);
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        bind.setAccessible(true);
        bind.invoke(Blocks.OAK_LOG.builtInRegistryHolder(), List.of(BlockTags.PREVENTS_NEARBY_LEAF_DECAY));
        var tick = LeavesBlock.class.getDeclaredMethod("tick", BlockState.class, ServerLevel.class, BlockPos.class, RandomSource.class);
        tick.setAccessible(true);
        var leaves = List.of(Blocks.OAK_LEAVES,Blocks.SPRUCE_LEAVES,Blocks.BIRCH_LEAVES,Blocks.JUNGLE_LEAVES,
            Blocks.ACACIA_LEAVES,Blocks.DARK_OAK_LEAVES,Blocks.MANGROVE_LEAVES,Blocks.CHERRY_LEAVES,
            Blocks.PALE_OAK_LEAVES,Blocks.AZALEA_LEAVES,Blocks.FLOWERING_AZALEA_LEAVES);
        var neighbors = List.of(Blocks.STONE.defaultBlockState(), Blocks.OAK_LOG.defaultBlockState(),
            Blocks.OAK_LEAVES.defaultBlockState().setValue(LeavesBlock.DISTANCE, 2),
            Blocks.OAK_LEAVES.defaultBlockState().setValue(LeavesBlock.DISTANCE, 6));
        out.println("BEGIN_RESULT");
        for (var block : leaves) for (int distance : new int[]{1,4,7}) for (boolean wet : new boolean[]{false,true})
            for (int neighbor = 0; neighbor < neighbors.size(); neighbor++) for (var direction : Direction.values()) {
                blocks.clear(); blocks.put(CENTER.relative(direction), neighbors.get(neighbor));
                var current = block.defaultBlockState().setValue(LeavesBlock.DISTANCE,distance).setValue(LeavesBlock.WATERLOGGED,wet);
                result = null;
                tick.invoke(block,current,level,CENTER,RandomSource.create(42));
                out.printf("%s %d %s %d %s %d %s%n", BuiltInRegistries.BLOCK.getKey(block),distance,wet,neighbor,
                    direction.getName(),result.getValue(LeavesBlock.DISTANCE),result.getValue(LeavesBlock.WATERLOGGED));
            }
        out.println("END_RESULT");
    }
}
