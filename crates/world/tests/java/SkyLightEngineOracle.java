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
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.block.state.properties.Half;
import net.minecraft.world.level.block.state.properties.StairsShape;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.lighting.*;
import net.minecraft.world.level.material.FluidState;

// Java performs all sky-light propagation; Rust reads only the resulting grid.
public class SkyLightEngineOracle implements LightChunkGetter, BlockGetter {
    private final Map<BlockPos, BlockState> blocks = new HashMap<>();
    private final Map<ChunkPos, TestChunk> chunks = new HashMap<>();
    int minY = 0, height = 32;
    public int getMinY() { return minY; }
    public int getHeight() { return height; }
    public BlockGetter getLevel() { return this; }
    public LightChunk getChunkForLighting(int x, int z) { return chunks.get(new ChunkPos(x, z)); }
    public BlockEntity getBlockEntity(BlockPos p) { return null; }
    public BlockState getBlockState(BlockPos p) { return blocks.getOrDefault(p, Blocks.AIR.defaultBlockState()); }
    public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
    class TestChunk implements LightChunk {
        final int chunkX, chunkZ;
        TestChunk(int x, int z) { chunkX = x; chunkZ = z; }
        final ChunkSkyLightSources sources = new ChunkSkyLightSources(SkyLightEngineOracle.this);
        public BlockState getBlockState(BlockPos p) { return SkyLightEngineOracle.this.getBlockState(new BlockPos(chunkX * 16 + Math.floorMod(p.getX(), 16), p.getY(), chunkZ * 16 + Math.floorMod(p.getZ(), 16))); }
        public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
        public BlockEntity getBlockEntity(BlockPos p) { return null; }
        public int getMinY() { return SkyLightEngineOracle.this.minY; }
        public int getHeight() { return SkyLightEngineOracle.this.height; }
        public ChunkSkyLightSources getSkyLightSources() { return sources; }
        public void findBlockLightSources(BiConsumer<BlockPos, BlockState> consumer) {}
    }
    private static BlockState state(String name) {
        return switch (name) {
            case "air" -> Blocks.AIR.defaultBlockState();
            case "stone" -> Blocks.STONE.defaultBlockState();
            case "water" -> Blocks.WATER.defaultBlockState();
            case "glass" -> Blocks.GLASS.defaultBlockState();
            case "slab_bottom" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM);
            case "slab_top" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP);
            case "wet_slab" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM).setValue(SlabBlock.WATERLOGGED, true);
            case "stair" -> Blocks.OAK_STAIRS.defaultBlockState().setValue(StairBlock.FACING, Direction.NORTH).setValue(StairBlock.HALF, Half.BOTTOM).setValue(StairBlock.SHAPE, StairsShape.STRAIGHT);
            case "snow_1" -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 1);
            case "snow_7" -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 7);
            case "snow_8" -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 8);
            default -> throw new IllegalArgumentException(name);
        };
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var actions = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var world = new SkyLightEngineOracle();
        if (args.length > 3) { world.minY = Integer.parseInt(args[2]); world.height = Integer.parseInt(args[3]); }
        var engine = new LevelLightEngine(world, false, true);
        for (int x = -1; x <= 2; x++) for (int z = -1; z <= 2; z++) {
            world.chunks.put(new ChunkPos(x, z), world.new TestChunk(x, z));
            for (int y = world.minY / 16; y < (world.minY + world.height) / 16; y++) engine.updateSectionStatus(SectionPos.of(x, y, z), false);
        }
        while (engine.hasLightWork()) engine.runLightUpdates();
        for (int x = -1; x <= 2; x++) for (int z = -1; z <= 2; z++) engine.propagateLightSources(new ChunkPos(x, z));
        while (engine.hasLightWork()) engine.runLightUpdates();
        var result = new JsonArray();
        for (var element : actions) {
            var item = element.getAsJsonObject();
            var p = new BlockPos(item.get("x").getAsInt(), item.get("y").getAsInt(), item.get("z").getAsInt());
            world.blocks.put(p, state(item.get("state").getAsString()));
            var chunk = world.chunks.get(new ChunkPos(Math.floorDiv(p.getX(), 16), Math.floorDiv(p.getZ(), 16)));
            chunk.sources.update(chunk, Math.floorMod(p.getX(), 16), p.getY(), Math.floorMod(p.getZ(), 16));
            engine.checkBlock(p);
            if (item.has("defer") && item.get("defer").getAsBoolean()) continue;
            while (engine.hasLightWork()) engine.runLightUpdates();
            var grid = new JsonArray();
            var listener = engine.getLayerListener(LightLayer.SKY);
            for (int y = world.minY - 15; y < world.minY + world.height + 15; y++) for (int z = 0; z < 31; z++) for (int x = 0; x < 31; x++)
                grid.add(listener.getLightValue(new BlockPos(x, y, z)));
            result.add(grid);
        }
        Files.writeString(Path.of(args[1]), result.toString());
    }
}
