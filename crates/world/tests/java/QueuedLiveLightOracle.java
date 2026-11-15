import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.SectionPos;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LightLayer;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.DataLayer;
import net.minecraft.world.level.lighting.LevelLightEngine;
import net.minecraft.world.level.lighting.LightEngine;

// Continue a queued saved-light revision with actual Java block checks.
public class QueuedLiveLightOracle extends SavedLiveLightOracle {
    public static void main(String[] args) throws Exception {
        QueuedSectionLightOracle.OUT.flush();
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var world = new QueuedLiveLightOracle();
        for (int x = -3; x <= 3; x++) for (int z = -3; z <= 3; z++) {
            var chunk = world.new TestChunk(x, z);
            world.chunks.put(new ChunkPos(x, z), chunk);
            world.blocks.put(new BlockPos(x * 16, 1, z * 16), Blocks.STONE.defaultBlockState());
            chunk.sources.update(chunk, 0, 1, 0);
        }
        var options = java.util.Arrays.asList(args);
        boolean sky = !options.contains("no-sky");
        boolean saved = !options.contains("no-saved");
        var engine = new LevelLightEngine(world, true, sky);
        if (saved) engine.retainData(new ChunkPos(0, 0), true);
        if (saved) engine.retainData(new ChunkPos(-1, 0), true);
        if (saved) engine.queueSectionData(LightLayer.BLOCK, SectionPos.of(0, 0, 0), new DataLayer(QueuedSectionLightOracle.bytes(0)));
        if (saved && sky) {
            engine.queueSectionData(LightLayer.SKY, SectionPos.of(0, 0, 0), new DataLayer(QueuedSectionLightOracle.bytes(2)));
            engine.queueSectionData(LightLayer.SKY, SectionPos.of(0, 2, 0), new DataLayer(QueuedSectionLightOracle.bytes(1)));
        }
        for (int x = -2; x <= 2; x++) for (int z = -2; z <= 2; z++)
            engine.updateSectionStatus(SectionPos.of(x, 0, z), false);
        drain(engine);
        engine.setLightEnabled(new ChunkPos(0, 0), true);
        engine.runLightUpdates();
        QueuedSectionLightOracle.capture(engine, true, sky, "0");
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
            if (saved && i == 0) engine.queueSectionData(LightLayer.BLOCK, SectionPos.of(0, 0, 0), new DataLayer(QueuedSectionLightOracle.bytes(0)));
            if (empty != nextEmpty) engine.updateSectionStatus(SectionPos.of(0, 1, 0), nextEmpty);
            empty = nextEmpty;
            if (LightEngine.hasDifferentLightProperties(old, next)) {
                world.chunks.get(new ChunkPos(0, 0)).sources.update(world.chunks.get(new ChunkPos(0, 0)), 8, 20, 8);
                engine.checkBlock(pos);
            }
            drain(engine);
            QueuedSectionLightOracle.capture(engine, true, sky, Integer.toString(i + 1));
        }
        engine.updateSectionStatus(SectionPos.of(0, 1, 0), false);
        engine.setLightEnabled(new ChunkPos(-1, 0), true);
        engine.runLightUpdates();
        QueuedSectionLightOracle.capture(engine, true, sky, "7");
        world.blocks.put(pos, Blocks.TORCH.defaultBlockState());
        world.chunks.get(new ChunkPos(0, 0)).sources.update(world.chunks.get(new ChunkPos(0, 0)), 8, 20, 8);
        engine.propagateLightSources(new ChunkPos(0, 0));
        engine.runLightUpdates();
        QueuedSectionLightOracle.capture(engine, true, sky, "8");
    }
}
