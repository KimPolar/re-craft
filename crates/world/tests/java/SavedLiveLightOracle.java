import java.util.HashMap;
import java.util.HexFormat;
import java.util.Map;
import java.util.function.BiConsumer;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.SectionPos;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LightLayer;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.LightChunk;
import net.minecraft.world.level.chunk.LightChunkGetter;
import net.minecraft.world.level.lighting.ChunkSkyLightSources;
import net.minecraft.world.level.lighting.LevelLightEngine;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.world.level.material.FluidState;

// A saved, source-enabled light revision followed by real section callbacks
// and block checks. Chunk registration and light drains are explicitly ordered.
public class SavedLiveLightOracle implements LightChunkGetter, BlockGetter {
    static final java.io.PrintStream OUT = System.out;
    final Map<BlockPos, BlockState> blocks = new HashMap<>();
    final Map<ChunkPos, TestChunk> chunks = new HashMap<>();
    public int getMinY() { return 0; }
    public int getHeight() { return 32; }
    public BlockGetter getLevel() { return this; }
    public LightChunk getChunkForLighting(int x, int z) { return chunks.get(new ChunkPos(x, z)); }
    public void onLightUpdate(LightLayer layer, SectionPos pos) {}
    public BlockEntity getBlockEntity(BlockPos pos) { return null; }
    public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState()); }
    public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    class TestChunk implements LightChunk {
        final int x, z;
        final ChunkSkyLightSources sources = new ChunkSkyLightSources(SavedLiveLightOracle.this);
        TestChunk(int x, int z) { this.x = x; this.z = z; }
        public int getMinY() { return 0; }
        public int getHeight() { return 32; }
        public BlockState getBlockState(BlockPos pos) {
            return SavedLiveLightOracle.this.getBlockState(new BlockPos(
                x * 16 + Math.floorMod(pos.getX(), 16), pos.getY(), z * 16 + Math.floorMod(pos.getZ(), 16)));
        }
        public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
        public BlockEntity getBlockEntity(BlockPos pos) { return null; }
        public ChunkSkyLightSources getSkyLightSources() { return sources; }
        public void findBlockLightSources(BiConsumer<BlockPos, BlockState> consumer) {
            blocks.forEach((pos, state) -> {
                if (Math.floorDiv(pos.getX(), 16) == x && Math.floorDiv(pos.getZ(), 16) == z
                    && state.getLightEmission() > 0) consumer.accept(pos, state);
            });
        }
    }
    static void drain(LevelLightEngine engine) { while (engine.hasLightWork()) engine.runLightUpdates(); }
    static void capture(LevelLightEngine engine, int stage) {
        OUT.printf("H %d%n", stage);
        for (var layer : new LightLayer[]{LightLayer.BLOCK, LightLayer.SKY}) {
            var listener = engine.getLayerListener(layer);
            for (int x = -2; x <= 2; x++) for (int z = -2; z <= 2; z++) for (int y = -1; y <= 2; y++) {
                var data = listener.getDataLayerData(SectionPos.of(x, y, z));
                if (data == null) continue;
                boolean saved = !data.isEmpty();
                OUT.printf("L %s %d %d %d %d %s%n", layer == LightLayer.BLOCK ? "B" : "S",
                    x, y, z, saved ? 1 : 0, HexFormat.of().formatHex(data.getData()));
            }
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var world = new SavedLiveLightOracle();
        for (int x = -3; x <= 3; x++) for (int z = -3; z <= 3; z++) {
            var chunk = world.new TestChunk(x, z);
            world.chunks.put(new ChunkPos(x, z), chunk);
            world.blocks.put(new BlockPos(x * 16, 1, z * 16), Blocks.STONE.defaultBlockState());
            chunk.sources.update(chunk, 0, 1, 0);
        }
        boolean hasSky = args.length == 0 || !args[0].equals("no-sky");
        var engine = new LevelLightEngine(world, true, hasSky);
        for (int x = -3; x <= 3; x++) for (int z = -3; z <= 3; z++)
            engine.updateSectionStatus(SectionPos.of(x, 0, z), false);
        drain(engine);
        engine.propagateLightSources(new ChunkPos(0, 0));
        drain(engine);
        capture(engine, 0);
        var pos = new BlockPos(8, 20, 8);
        BlockState[] states = {
            Blocks.TORCH.defaultBlockState(), Blocks.AIR.defaultBlockState(),
            Blocks.GLASS.defaultBlockState(), Blocks.AIR.defaultBlockState(),
            Blocks.STONE.defaultBlockState(), Blocks.AIR.defaultBlockState()
        };
        boolean empty = true;
        for (int i = 0; i < states.length; i++) {
            var old = world.getBlockState(pos);
            var next = states[i];
            world.blocks.put(pos, next);
            boolean nextEmpty = next.isAir();
            if (empty != nextEmpty) engine.updateSectionStatus(SectionPos.of(0, 1, 0), nextEmpty);
            empty = nextEmpty;
            if (LightEngine.hasDifferentLightProperties(old, next)) {
                world.chunks.get(new ChunkPos(0, 0)).sources.update(world.chunks.get(new ChunkPos(0, 0)), 8, 20, 8);
                engine.checkBlock(pos);
            }
            drain(engine);
            capture(engine, i + 1);
        }
    }
}
