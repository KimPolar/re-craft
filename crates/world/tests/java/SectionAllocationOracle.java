import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.JsonArray;
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

public class SectionAllocationOracle implements LightChunkGetter, BlockGetter {
    public int getMinY() { return 0; }
    public int getHeight() { return 16; }
    public BlockGetter getLevel() { return this; }
    public LightChunk getChunkForLighting(int x, int z) { return null; }
    public void onLightUpdate(LightLayer layer, SectionPos pos) {}
    public BlockEntity getBlockEntity(BlockPos pos) { return null; }
    public BlockState getBlockState(BlockPos pos) { return Blocks.AIR.defaultBlockState(); }
    public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    static void drain(LevelLightEngine engine) { while (engine.hasLightWork()) engine.runLightUpdates(); }
    static JsonArray capture(LevelLightEngine engine) {
        var columns = new JsonArray();
        for (int cz = -1; cz <= 1; cz++) for (int cx = -1; cx <= 2; cx++) {
            var channels = new JsonArray();
            for (var channel : new LightLayer[]{LightLayer.BLOCK, LightLayer.SKY}) {
                var values = new JsonArray();
                var listener = engine.getLayerListener(channel);
                for (int y = -16; y < 80; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++) {
                    values.add(listener.getLightValue(new BlockPos(cx * 16 + x, y, cz * 16 + z)));
                }
                channels.add(values);
            }
            columns.add(channels);
        }
        return columns;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var world = new SectionAllocationOracle();
        var engine = new LevelLightEngine(world, true, true);
        if (args.length > 1 && args[1].startsWith("active")) engine.setLightEnabled(new ChunkPos(2, 0), true);
        engine.updateSectionStatus(SectionPos.of(0, 3, 0), false);
        drain(engine);
        var upper = new DataLayer();
        for (int y = 0; y < 16; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++) {
            upper.set(x, y, z, (x * 3 + z * 5 + y * 7) & 15);
        }
        engine.queueSectionData(LightLayer.SKY, SectionPos.of(0, 2, 0), upper);
        drain(engine);
        var stages = new JsonArray();
        stages.add(capture(engine));
        boolean many=args.length > 1 && args[1].contains("many");
        boolean reverse=args.length > 1 && args[1].endsWith("reverse");
        if(many && reverse) engine.updateSectionStatus(SectionPos.of(1, 1, 0), false);
        engine.updateSectionStatus(SectionPos.of(1, 0, 0), false);
        if(many && !reverse) engine.updateSectionStatus(SectionPos.of(1, 1, 0), false);
        stages.add(capture(engine));
        drain(engine);
        stages.add(capture(engine));
        Files.writeString(Path.of(args[0]), stages.toString());
    }
}
