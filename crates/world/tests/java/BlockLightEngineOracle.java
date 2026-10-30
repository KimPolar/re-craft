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
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.block.state.properties.Half;
import net.minecraft.world.level.block.state.properties.StairsShape;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.lighting.*;
import net.minecraft.world.level.material.FluidState;

// A small in-memory block world; propagation itself is performed by Java 26.3.
public class BlockLightEngineOracle implements LightChunkGetter, LightChunk {
    private final Map<BlockPos, BlockState> blocks = new HashMap<>();
    public int getHeight() { return 64; }
    public int getMinY() { return -16; }
    public LightChunk getChunkForLighting(int x, int z) { return this; }
    public BlockGetter getLevel() { return this; }
    public BlockEntity getBlockEntity(BlockPos p) { return null; }
    public BlockState getBlockState(BlockPos p) { return blocks.getOrDefault(p, Blocks.AIR.defaultBlockState()); }
    public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
    public ChunkSkyLightSources getSkyLightSources() { return null; }
    public void findBlockLightSources(BiConsumer<BlockPos, BlockState> consumer) {
        blocks.forEach((p, s) -> { if (s.getLightEmission() > 0) consumer.accept(p, s); });
    }
    private static BlockPos position(JsonObject v) {
        return new BlockPos(v.get("x").getAsInt(), v.get("y").getAsInt(), v.get("z").getAsInt());
    }
    private static BlockState state(String name) {
        return switch (name) {
            case "air" -> Blocks.AIR.defaultBlockState();
            case "stone" -> Blocks.STONE.defaultBlockState();
            case "torch" -> Blocks.TORCH.defaultBlockState();
            case "water" -> Blocks.WATER.defaultBlockState();
            case "wet_pane" -> Blocks.GLASS_PANE.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED, true);
            case "slab_bottom" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM);
            case "slab_top" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP);
            case "stair_bottom" -> Blocks.OAK_STAIRS.defaultBlockState().setValue(StairBlock.FACING, Direction.NORTH).setValue(StairBlock.HALF, Half.BOTTOM).setValue(StairBlock.SHAPE, StairsShape.STRAIGHT);
            case "stair_top" -> Blocks.OAK_STAIRS.defaultBlockState().setValue(StairBlock.FACING, Direction.EAST).setValue(StairBlock.HALF, Half.TOP).setValue(StairBlock.SHAPE, StairsShape.INNER_LEFT);
            case "snow_1" -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 1);
            case "snow_7" -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 7);
            case "snow_8" -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 8);
            case "farmland" -> Blocks.FARMLAND.defaultBlockState();
            case "dirt_path" -> Blocks.DIRT_PATH.defaultBlockState();
            case "spruce_log" -> Blocks.SPRUCE_LOG.defaultBlockState().setValue(RotatedPillarBlock.AXIS, Direction.Axis.X);
            case "birch_leaves" -> Blocks.BIRCH_LEAVES.defaultBlockState().setValue(LeavesBlock.DISTANCE, 3).setValue(LeavesBlock.PERSISTENT, true);
            case "ice" -> Blocks.ICE.defaultBlockState();
            case "podzol" -> Blocks.PODZOL.defaultBlockState().setValue(BlockStateProperties.SNOWY, true);
            case "brown_mushroom" -> Blocks.BROWN_MUSHROOM.defaultBlockState();
            case "unlit" -> Blocks.REDSTONE_ORE.defaultBlockState().setValue(RedStoneOreBlock.LIT, false);
            case "lit" -> Blocks.REDSTONE_ORE.defaultBlockState().setValue(RedStoneOreBlock.LIT, true);
            default -> throw new IllegalArgumentException(name);
        };
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
        var world = new BlockLightEngineOracle();
        var engine = new LevelLightEngine(world, true, false);
        for (int x = -1; x <= 2; x++) for (int z = -1; z <= 2; z++) {
            for (int y = -1; y <= 2; y++) engine.updateSectionStatus(SectionPos.of(x, y, z), false);
            engine.setLightEnabled(new ChunkPos(x, z), true);
        }
        while (engine.hasLightWork()) engine.runLightUpdates();
        for (var element : input.getAsJsonArray("initial")) {
            var item = element.getAsJsonObject();
            world.blocks.put(position(item), state(item.get("state").getAsString()));
        }
        for (var element : input.getAsJsonArray("initial")) engine.checkBlock(position(element.getAsJsonObject()));
        while (engine.hasLightWork()) engine.runLightUpdates();
        var results = new JsonArray();
        for (var element : input.getAsJsonArray("actions")) {
            var item = element.getAsJsonObject();
            var pos = position(item);
            world.blocks.put(pos, state(item.get("state").getAsString()));
            engine.checkBlock(pos);
            if (item.has("defer") && item.get("defer").getAsBoolean()) continue;
            while (engine.hasLightWork()) engine.runLightUpdates();
            var grid = new JsonArray();
            var listener = engine.getLayerListener(LightLayer.BLOCK);
            for (int y = 0; y < 31; y++) for (int z = 0; z < 31; z++) for (int x = 0; x < 31; x++)
                grid.add(listener.getLightValue(new BlockPos(x, y, z)));
            results.add(grid);
        }
        Files.writeString(Path.of(args[1]), results.toString());
    }
}
