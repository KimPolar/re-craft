import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.Strategy;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.lighting.ChunkSkyLightSources;
import net.minecraft.world.level.lighting.LightEngine;

public class GeneratedSkySourcesReplayOracle {
    static final java.io.PrintStream OUT = System.out;
    static class FakeChunk extends ProtoChunk {
        LevelChunkSection[] sections;
        FakeChunk() { super(null, null, null, null, null); }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 32; }
        @Override public LevelChunkSection getSection(int index) { return sections[index]; }
        @Override public int getHighestFilledSectionIndex() {
            for (int i = sections.length - 1; i >= 0; i--)
                if (!sections[i].hasOnlyAir()) return i;
            return -1;
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        int glassCount = 0;
        for (var block : net.minecraft.core.registries.BuiltInRegistries.BLOCK) {
            var name = net.minecraft.core.registries.BuiltInRegistries.BLOCK.getKey(block).toString();
            if (name.equals("minecraft:glass") || name.endsWith("_stained_glass")) {
                glassCount++;
                var glassState = block.defaultBlockState();
                if (glassState.getValues().count() != 0
                    || LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(), glassState))
                    throw new AssertionError("glass unexpectedly requested a light check: " + name);
            }
        }
        if (glassCount != 17) throw new AssertionError("unexpected glass registry count: " + glassCount);
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var chunk = (FakeChunk)((sun.misc.Unsafe)field.get(null)).allocateInstance(FakeChunk.class);
        chunk.sections = new LevelChunkSection[2];
        for (int i = 0; i < 2; i++) {
            var blocks = new PalettedContainer<>(Blocks.AIR.defaultBlockState(),
                Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
            chunk.sections[i] = new LevelChunkSection(blocks, null);
        }
        chunk.sections[0].setBlockState(1, 5, 1, Blocks.GLASS.defaultBlockState());
        chunk.sections[1].setBlockState(8, 0, 8,
            Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM));
        var sources = new ChunkSkyLightSources(chunk);
        sources.fillFrom(chunk);
        int before = sources.getLowestSourceY(8, 8);
        var previous = chunk.sections[0].setBlockState(1, 5, 1, Blocks.AIR.defaultBlockState());
        if (LightEngine.hasDifferentLightProperties(previous, Blocks.AIR.defaultBlockState()))
            throw new AssertionError("glass removal unexpectedly requested a light check");
        int after = sources.getLowestSourceY(8, 8);
        var rescanned = new ChunkSkyLightSources(chunk);
        rescanned.fillFrom(chunk);
        OUT.println("S " + before + " " + after + " " + rescanned.getLowestSourceY(8, 8));

        for (int i = 0; i < 2; i++) {
            var blocks = new PalettedContainer<>(Blocks.AIR.defaultBlockState(),
                Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
            chunk.sections[i] = new LevelChunkSection(blocks, null);
        }
        var checked = new ChunkSkyLightSources(chunk);
        checked.fillFrom(chunk);
        int initial = checked.getLowestSourceY(15, 15);
        var slab = Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM);
        previous = chunk.sections[0].setBlockState(15, 0, 15, slab);
        if (!LightEngine.hasDifferentLightProperties(previous, slab))
            throw new AssertionError("slab did not request a light check");
        checked.update(chunk, 15, 0, 15);
        var checkedRescan = new ChunkSkyLightSources(chunk);
        checkedRescan.fillFrom(chunk);
        OUT.println("C " + initial + " " + checked.getLowestSourceY(15, 15) + " "
            + checkedRescan.getLowestSourceY(15, 15));

        for (int i = 0; i < 2; i++) {
            var blocks = new PalettedContainer<>(Blocks.AIR.defaultBlockState(),
                Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
            chunk.sections[i] = new LevelChunkSection(blocks, null);
        }
        chunk.sections[1].setBlockState(8, 0, 8, slab);
        var neutral = new ChunkSkyLightSources(chunk);
        neutral.fillFrom(chunk);
        var forced = new ChunkSkyLightSources(chunk);
        forced.fillFrom(chunk);
        int neutralBefore = neutral.getLowestSourceY(8, 8);
        var glass = Blocks.GLASS.defaultBlockState();
        previous = chunk.sections[0].setBlockState(1, 5, 1, glass);
        if (LightEngine.hasDifferentLightProperties(previous, glass))
            throw new AssertionError("first glass unexpectedly requested a light check");
        previous = chunk.sections[0].setBlockState(8, 15, 8, glass);
        if (LightEngine.hasDifferentLightProperties(previous, glass))
            throw new AssertionError("second glass unexpectedly requested a light check");
        forced.update(chunk, 8, 15, 8);
        OUT.println("N " + neutralBefore + " " + neutral.getLowestSourceY(8, 8) + " "
            + forced.getLowestSourceY(8, 8));
    }
}
