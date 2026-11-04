import java.lang.reflect.*;
import java.util.*;
import it.unimi.dsi.fastutil.shorts.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.ticks.*;

public class GeneratedLeafPostprocessingOracle {
    static BlockState leaf;
    static LevelTicks<Block> access;
    static long counter;
    static final BlockPos pos = new BlockPos(8, 8, 8);
    static void set(Object object, String name, Object value) throws Exception {
        for (Class<?> type = object.getClass(); type != null; type = type.getSuperclass()) {
            try { var f = type.getDeclaredField(name); f.setAccessible(true); f.set(object, value); return; }
            catch (NoSuchFieldException ignored) {}
        }
        throw new NoSuchFieldException(name);
    }
    static class FakeLevel extends ServerLevel {
        FakeLevel() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public BlockState getBlockState(BlockPos p) { return p.equals(pos) ? leaf : Blocks.STONE.defaultBlockState(); }
        @Override public RandomSource getRandom() { return RandomSource.create(42); }
        @Override public LevelTicks<Block> getBlockTicks() { return access; }
        @Override public long getGameTime() { return 100; }
        @Override public long nextSubTickCount() { return counter++; }
    }
    static class FakeChunk extends LevelChunk {
        FakeChunk() { super((Level)null,new ChunkPos(0,0)); }
        @Override public ChunkPos getPos() { return new ChunkPos(0,0); }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 16; }
        @Override public BlockState getBlockState(BlockPos p) { return p.equals(pos) ? leaf : Blocks.STONE.defaultBlockState(); }
    }
    static class FakeUpgrade extends UpgradeData {
        FakeUpgrade() { super(new CompoundTag(), LevelHeightAccessor.create(0,16)); }
        @Override public void upgrade(LevelChunk chunk) {}
    }
    public static void main(String[] args) throws Exception {
        var out = System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) f.get(null);
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class); bind.setAccessible(true);
        var level = (FakeLevel)unsafe.allocateInstance(FakeLevel.class);
        var chunk = (FakeChunk)unsafe.allocateInstance(FakeChunk.class);
        set(chunk,"upgradeData",unsafe.allocateInstance(FakeUpgrade.class));
        var leaves = List.of(Blocks.OAK_LEAVES,Blocks.SPRUCE_LEAVES,Blocks.BIRCH_LEAVES,Blocks.JUNGLE_LEAVES,
            Blocks.ACACIA_LEAVES,Blocks.DARK_OAK_LEAVES,Blocks.MANGROVE_LEAVES,Blocks.CHERRY_LEAVES,
            Blocks.PALE_OAK_LEAVES,Blocks.AZALEA_LEAVES,Blocks.FLOWERING_AZALEA_LEAVES);
        out.println("BEGIN_RESULT");
        for (var block : leaves) for (int distance : new int[]{1,4,7}) for (boolean tagged : new boolean[]{false,true}) {
            bind.invoke(Blocks.STONE.builtInRegistryHolder(), tagged ? List.of(BlockTags.PREVENTS_NEARBY_LEAF_DECAY) : List.of());
            leaf = block.defaultBlockState().setValue(LeavesBlock.DISTANCE,distance);
            ShortList[] marks = {new ShortArrayList()};
            marks[0].add(ProtoChunk.packOffsetCoordinates(pos)); marks[0].add(ProtoChunk.packOffsetCoordinates(pos));
            set(chunk,"postProcessing",marks); set(chunk,"pendingBlockEntities",new HashMap<BlockPos,CompoundTag>());
            var ticks = new LevelChunkTicks<Block>(List.of()); access = new LevelTicks<Block>(p -> true);
            access.addContainer(new ChunkPos(0,0), ticks); counter = 2;
            chunk.postProcessGeneration(level);
            var saved = ticks.pack(100);
            out.printf("%s %d %s %d %d %d%n", BuiltInRegistries.BLOCK.getKey(block),distance,tagged,
                saved.size(),saved.isEmpty() ? -1 : saved.get(0).delay(),counter);
        }
        bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of());
        out.println("END_RESULT");
    }
}
