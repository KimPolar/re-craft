import java.nio.file.*;
import java.util.*;
import java.util.function.BiConsumer;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.lighting.*;
import net.minecraft.world.level.material.FluidState;

// Compare saved-layer omission with the live Java 26.3 sky listener.
public class SkyStorageOracle implements LightChunkGetter, BlockGetter {
    final Map<BlockPos, BlockState> blocks = new HashMap<>();
    final Map<ChunkPos, TestChunk> chunks = new HashMap<>();
    public int getMinY() { return 0; }
    public int getHeight() { return 64; }
    public BlockGetter getLevel() { return this; }
    public LightChunk getChunkForLighting(int x, int z) { return chunks.get(new ChunkPos(x,z)); }
    public BlockEntity getBlockEntity(BlockPos p) { return null; }
    public BlockState getBlockState(BlockPos p) { return blocks.getOrDefault(p, Blocks.AIR.defaultBlockState()); }
    public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
    class TestChunk implements LightChunk {
        final int x,z;
        final ChunkSkyLightSources sources = new ChunkSkyLightSources(SkyStorageOracle.this);
        TestChunk(int x, int z) { this.x=x; this.z=z; }
        public int getMinY() { return 0; }
        public int getHeight() { return 64; }
        public BlockState getBlockState(BlockPos p) { return SkyStorageOracle.this.getBlockState(new BlockPos(x*16+Math.floorMod(p.getX(),16),p.getY(),z*16+Math.floorMod(p.getZ(),16))); }
        public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
        public BlockEntity getBlockEntity(BlockPos p) { return null; }
        public ChunkSkyLightSources getSkyLightSources() { return sources; }
        public void findBlockLightSources(BiConsumer<BlockPos,BlockState> consumer) {}
    }
    static JsonObject run(boolean roof) {
        var world = new SkyStorageOracle();
        for (int x=-2;x<=2;x++) for (int z=-2;z<=2;z++) world.chunks.put(new ChunkPos(x,z),world.new TestChunk(x,z));
        if (roof) for (int x=-16;x<32;x++) for (int z=-16;z<32;z++) {
            var p = new BlockPos(x,31,z);
            world.blocks.put(p,Blocks.STONE.defaultBlockState());
            var chunk = world.chunks.get(new ChunkPos(Math.floorDiv(x,16),Math.floorDiv(z,16)));
            chunk.sources.update(chunk,Math.floorMod(x,16),31,Math.floorMod(z,16));
        }
        var engine = new LevelLightEngine(world,false,true);
        if (roof) for (int x=-1;x<=1;x++) for (int z=-1;z<=1;z++) engine.updateSectionStatus(SectionPos.of(x,1,z),false);
        while (engine.hasLightWork()) engine.runLightUpdates();
        for (int x=-2;x<=2;x++) for (int z=-2;z<=2;z++) engine.propagateLightSources(new ChunkPos(x,z));
        while (engine.hasLightWork()) engine.runLightUpdates();
        var listener = engine.getLayerListener(LightLayer.SKY);
        var noSky = new LevelLightEngine(world,false,false).getLayerListener(LightLayer.SKY);
        var above = new JsonArray();
        var noSkyValues = new JsonArray();
        for (int y : new int[]{80, 81, 320, 1000}) {
            above.add(listener.getLightValue(new BlockPos(8,y,8)));
            noSkyValues.add(noSky.getLightValue(new BlockPos(8,y,8)));
        }
        var layers = new JsonArray();
        for (int x=-2;x<=2;x++) for (int z=-2;z<=2;z++) for (int sy=-1;sy<=4;sy++) {
            var data = listener.getDataLayerData(SectionPos.of(x,sy,z));
            var row = new JsonObject(); row.addProperty("x",x); row.addProperty("z",z); row.addProperty("y",sy);
            if (data == null || data.isEmpty()) row.add("saved",JsonNull.INSTANCE);
            else { var values=new JsonArray(); for (byte b:data.getData()) values.add(b & 255); row.add("saved",values); }
            layers.add(row);
        }
        var grid = new JsonArray();
        for (int y=-16;y<80;y++) for (int z=0;z<16;z++) for (int x=0;x<16;x++) grid.add(listener.getLightValue(new BlockPos(x,y,z)));
        var p = new BlockPos(8,63,8);
        world.blocks.put(p,Blocks.STONE.defaultBlockState());
        var chunk = world.chunks.get(new ChunkPos(0,0));
        chunk.sources.update(chunk,8,63,8);
        engine.updateSectionStatus(SectionPos.of(0,3,0),false);
        engine.checkBlock(p);
        while (engine.hasLightWork()) engine.runLightUpdates();
        var after = new JsonArray();
        for (int y=-16;y<80;y++) for (int z=0;z<16;z++) for (int x=0;x<16;x++) after.add(listener.getLightValue(new BlockPos(x,y,z)));
        var result = new JsonObject(); result.add("layers",layers); result.add("grid",grid); result.add("after",after); result.add("above",above); result.add("noSky",noSkyValues); return result;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var result = new JsonArray(); result.add(run(false)); result.add(run(true));
        Files.writeString(Path.of(args[0]), result.toString());
    }
}
