import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.SectionPos;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LightLayer;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.chunk.DataLayer;
import net.minecraft.world.level.lighting.LevelLightEngine;

// Register a light-correct saved chunk after its neighbor is already active.
public class StreamedSavedLightOracle extends SavedLiveLightOracle {
    static void load(StreamedSavedLightOracle world, LevelLightEngine engine,
                     int x, boolean sky, boolean separate, int blockPattern) {
        var pos = new ChunkPos(x, 0);
        var chunk = world.new TestChunk(x, 0);
        world.chunks.put(pos, chunk);
        world.blocks.put(new BlockPos(x * 16, 1, 0), Blocks.STONE.defaultBlockState());
        chunk.sources.update(chunk, 0, 1, 0);
        engine.retainData(pos, true);
        engine.queueSectionData(LightLayer.BLOCK, SectionPos.of(x, 0, 0),
            new DataLayer(QueuedSectionLightOracle.bytes(blockPattern)));
        if (sky) engine.queueSectionData(LightLayer.SKY, SectionPos.of(x, 0, 0),
            new DataLayer(QueuedSectionLightOracle.bytes(2)));
        engine.queueSectionData(LightLayer.BLOCK, SectionPos.of(x, 2, 0),
            new DataLayer(QueuedSectionLightOracle.bytes(3)));
        if (separate) engine.runLightUpdates();
        engine.updateSectionStatus(SectionPos.of(x, 0, 0), false);
        engine.runLightUpdates();
        engine.setLightEnabled(pos, true);
        engine.retainData(pos, false);
        engine.runLightUpdates();
    }

    public static void main(String[] args) throws Exception {
        QueuedSectionLightOracle.OUT.flush();
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        int index = 0;
        for (boolean sky : new boolean[]{true, false}) {
            for (boolean separate : new boolean[]{true, false}) {
                var world = new StreamedSavedLightOracle();
                var engine = new LevelLightEngine(world, true, sky);
                load(world, engine, 0, sky, separate, 1);
                QueuedSectionLightOracle.capture(engine, true, sky, index + "_before");
                load(world, engine, 1, sky, separate, 3);
                QueuedSectionLightOracle.capture(engine, true, sky, index + "_after");
                index++;
            }
        }
    }
}
