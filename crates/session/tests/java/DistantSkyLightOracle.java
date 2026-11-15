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

    static void put(SavedLiveLightOracle world, BlockPos pos, net.minecraft.world.level.block.state.BlockState state) {
        world.blocks.put(pos, state);
        var chunk = world.chunks.get(new ChunkPos(Math.floorDiv(pos.getX(), 16), Math.floorDiv(pos.getZ(), 16)));
        chunk.sources.update(chunk, Math.floorMod(pos.getX(), 16), pos.getY(), Math.floorMod(pos.getZ(), 16));
    }

    static void run(DataOutputStream out, boolean emptyTarget, int editX, int editY, int editZ,
                    boolean water, boolean roof, boolean cave) throws Exception {
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
        if (roof || cave) {
            int roofY = cave ? 24 : editY;
            for (int x = -2; x <= 4; x++) for (int z = -2; z <= 3; z++) {
                var chunk = world.chunks.get(new ChunkPos(x, z));
                for (int localX = 0; localX < 16; localX++) for (int localZ = 0; localZ < 16; localZ++) {
                    world.blocks.put(new BlockPos(x * 16 + localX, roofY, z * 16 + localZ), Blocks.STONE.defaultBlockState());
                    chunk.sources.update(chunk, localX, roofY, localZ);
                }
            }
        }
        if (cave) {
            for (int x = -2; x <= 18; x++) for (int z = -2; z <= 2; z++) for (int y = 0; y < 24; y++)
                put(world, new BlockPos(x, y, z), Blocks.STONE.defaultBlockState());
            for (int y = 0; y <= 24; y++) put(world, new BlockPos(0, y, 0), Blocks.AIR.defaultBlockState());
            for (int x = 1; x <= 8; x++) put(world, new BlockPos(x, 8, 0), Blocks.AIR.defaultBlockState());
            for (int y = 8; y <= 18; y++) put(world, new BlockPos(8, y, 0), Blocks.AIR.defaultBlockState());
            put(world, new BlockPos(editX, editY, editZ), Blocks.STONE.defaultBlockState());
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
        var position = new BlockPos(editX, editY, editZ);
        var center = SectionPos.of(position);
        var listener = engine.getLayerListener(LightLayer.SKY);
        if (emptyTarget && listener.getDataLayerData(center) != null)
            throw new IllegalStateException("target light section must initially be unallocated");
        capture(out, engine);

        var chunk = world.chunks.get(new ChunkPos(Math.floorDiv(editX, 16), Math.floorDiv(editZ, 16)));
        for (boolean placing : new boolean[]{true, false}) {
            boolean solid = roof || cave ? !placing : placing;
            world.blocks.put(position, solid ? (water ? Blocks.WATER.defaultBlockState() : Blocks.STONE.defaultBlockState()) : Blocks.AIR.defaultBlockState());
            if (emptyTarget) engine.updateSectionStatus(center, !placing);
            chunk.sources.update(chunk, Math.floorMod(editX, 16), editY, Math.floorMod(editZ, 16));
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
            run(out, false, 0, 15, 0, false, false, false);
            run(out, true, 0, 15, 0, false, false, false);
            run(out, false, 15, 0, -1, false, false, false);
            run(out, false, -1, 24, 0, true, false, false);
            run(out, true, 0, 24, 0, false, false, false);
            run(out, false, 16, 8, 15, true, false, false);
            run(out, false, 0, 0, 0, false, false, false);
            run(out, false, 0, 16, 0, false, true, false);
            run(out, false, -1, 8, 0, false, true, false);
            run(out, false, 1, 8, 0, false, false, true);
        }
    }
}
