import java.util.HexFormat;
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
import net.minecraft.world.level.chunk.DataLayer;
import net.minecraft.world.level.chunk.LightChunk;
import net.minecraft.world.level.chunk.LightChunkGetter;
import net.minecraft.world.level.lighting.LevelLightEngine;
import net.minecraft.world.level.material.FluidState;

// Save visible layers using SerializableChunkData's null/isEmpty rule and
// compare later section callbacks against the same Java light engine.
public class SavedSectionLightOracle implements LightChunkGetter, BlockGetter {
    static final java.io.PrintStream OUT = System.out;
    public int getMinY() { return 0; }
    public int getHeight() { return 160; }
    public BlockGetter getLevel() { return this; }
    public LightChunk getChunkForLighting(int x, int z) { return null; }
    public void onLightUpdate(LightLayer layer, SectionPos pos) {}
    public BlockEntity getBlockEntity(BlockPos pos) { return null; }
    public BlockState getBlockState(BlockPos pos) { return Blocks.AIR.defaultBlockState(); }
    public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    static void drain(LevelLightEngine engine) { while (engine.hasLightWork()) engine.runLightUpdates(); }
    static void set(LevelLightEngine engine, int x, int y, int z, boolean nonempty) {
        engine.updateSectionStatus(SectionPos.of(x, y, z), !nonempty);
    }
    static void capture(LevelLightEngine engine, int test, String stage) {
        OUT.printf("H %d %s%n", test, stage);
        for (var layer : new LightLayer[]{LightLayer.BLOCK, LightLayer.SKY}) {
            var listener = engine.getLayerListener(layer);
            for (int x = -1; x <= 2; x++) for (int z = -1; z <= 1; z++) for (int y = -1; y <= 10; y++) {
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
        for (int test = 0; test < 6; test++) {
            var engine = new LevelLightEngine(new SavedSectionLightOracle(), true, true);
            set(engine, 0, 0, 0, true);
            set(engine, 1, 0, 0, true);
            set(engine, 0, 8, 0, true);
            drain(engine);
            engine.setLightEnabled(new ChunkPos(0, 0), true);
            engine.setLightEnabled(new ChunkPos(2, 0), true);
            for (var layer : new LightLayer[]{LightLayer.BLOCK, LightLayer.SKY}) {
                var data = new DataLayer();
                for (int y = 0; y < 16; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++) {
                    data.set(x, y, z, (x * 3 + y * 5 + z * 7 + (layer == LightLayer.SKY ? 4 : 0)) & 15);
                }
                engine.queueSectionData(layer, SectionPos.of(0, 8, 0), data);
            }
            // Explicit zero bytes are saved, unlike an unmaterialized zero layer.
            engine.queueSectionData(LightLayer.BLOCK, SectionPos.of(1, 0, 0), new DataLayer(new byte[2048]));
            engine.queueSectionData(LightLayer.SKY, SectionPos.of(0, 7, 0), new DataLayer(15));
            drain(engine);
            capture(engine, test, "before");
            switch (test) {
                case 0 -> set(engine, 0, 8, 0, false);
                case 1 -> { set(engine, 0, 8, 0, false); set(engine, 0, 8, 0, true); }
                case 2 -> { set(engine, 1, 4, 0, true); set(engine, 1, 4, 0, false); }
                case 3 -> set(engine, 0, 0, 0, false);
                case 4 -> { set(engine, 0, 0, 0, false); set(engine, 1, 0, 0, false); }
                case 5 -> set(engine, 1, 4, 0, true);
                default -> throw new AssertionError(test);
            }
            drain(engine);
            capture(engine, test, "after");
        }
    }
}
