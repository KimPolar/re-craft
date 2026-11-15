import java.io.BufferedOutputStream;
import java.io.DataOutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.SectionPos;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LightLayer;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.lighting.LevelLightEngine;

// Compare light before and after an edit, including a real empty-section
// registration and removal. Each byte is one queried Java sky-light value.
public class DistantSkyLightOracle {
    static void capture(DataOutputStream out, LevelLightEngine engine) throws Exception {
        var listener = engine.getLayerListener(LightLayer.SKY);
        for (int y = -1; y <= 32; y++)
            for (int z = -32; z < 64; z++)
                for (int x = -32; x < 80; x++)
                    out.writeByte(listener.getLightValue(new BlockPos(x, y, z)));
    }

    static void run(DataOutputStream out, boolean emptyTarget) throws Exception {
        var world = new SavedLiveLightOracle();
        for (int x = -2; x <= 4; x++) for (int z = -2; z <= 3; z++) {
            var chunk = world.new TestChunk(x, z);
            world.chunks.put(new ChunkPos(x, z), chunk);
            for (int sectionY = 0; sectionY <= 1; sectionY++) {
                if (emptyTarget && Math.abs(x) <= 1 && Math.abs(z) <= 1) continue;
                int y = sectionY * 16 + 1;
                world.blocks.put(new BlockPos(x * 16 + 15, y, z * 16 + 15), Blocks.STONE.defaultBlockState());
                chunk.sources.update(chunk, 15, y, 15);
            }
        }
        var engine = new LevelLightEngine(world, false, true);
        for (int x = -2; x <= 4; x++) for (int z = -2; z <= 3; z++)
            for (int sectionY = 0; sectionY <= 1; sectionY++)
                if (!(emptyTarget && Math.abs(x) <= 1 && Math.abs(z) <= 1))
                    engine.updateSectionStatus(SectionPos.of(x, sectionY, z), false);
        SavedLiveLightOracle.drain(engine);
        for (int x = -2; x <= 4; x++) for (int z = -2; z <= 3; z++)
            engine.propagateLightSources(new ChunkPos(x, z));
        SavedLiveLightOracle.drain(engine);
        var center = SectionPos.of(0, 0, 0);
        var listener = engine.getLayerListener(LightLayer.SKY);
        if (emptyTarget && listener.getDataLayerData(center) != null)
            throw new IllegalStateException("target light section must initially be unallocated");
        capture(out, engine);

        var position = new BlockPos(0, 15, 0);
        var chunk = world.chunks.get(new ChunkPos(0, 0));
        for (boolean placing : new boolean[]{true, false}) {
            world.blocks.put(position, placing ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState());
            if (emptyTarget) engine.updateSectionStatus(SectionPos.of(0, 0, 0), !placing);
            chunk.sources.update(chunk, 0, 15, 0);
            engine.checkBlock(position);
            SavedLiveLightOracle.drain(engine);
            if (emptyTarget && placing && listener.getDataLayerData(center) == null)
                throw new IllegalStateException("target light section must be allocated after placement");
            capture(out, engine);
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        try (var out = new DataOutputStream(new BufferedOutputStream(Files.newOutputStream(Path.of(args[0]))))) {
            run(out, false);
            run(out, true);
        }
    }
}
