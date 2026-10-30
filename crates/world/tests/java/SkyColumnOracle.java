import java.nio.file.*;
import java.util.*;
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
import net.minecraft.world.level.lighting.ChunkSkyLightSources;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.chunk.*;

public class SkyColumnOracle implements BlockGetter {
    static class FakeChunk extends ProtoChunk {
        LevelChunkSection[] sections;
        FakeChunk() { super(null, null, null, null, null); }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 32; }
        @Override public LevelChunkSection getSection(int index) { return sections[index]; }
        @Override public int getHighestFilledSectionIndex() {
            for (int i = sections.length - 1; i >= 0; --i)
                if (!sections[i].hasOnlyAir()) return i;
            return -1;
        }
    }
    final Map<BlockPos, BlockState> blocks = new HashMap<>();
    public int getHeight() { return 32; }
    public int getMinY() { return 0; }
    public BlockEntity getBlockEntity(BlockPos p) { return null; }
    public BlockState getBlockState(BlockPos p) { return blocks.getOrDefault(p, Blocks.AIR.defaultBlockState()); }
    public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
    private static BlockState state(String name) {
        return switch (name) {
            case "air" -> Blocks.AIR.defaultBlockState();
            case "stone" -> Blocks.STONE.defaultBlockState();
            case "water" -> Blocks.WATER.defaultBlockState();
            case "glass" -> Blocks.GLASS.defaultBlockState();
            case "torch" -> Blocks.TORCH.defaultBlockState();
            case "mushroom" -> Blocks.BROWN_MUSHROOM.defaultBlockState();
            case "slab_bottom" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.BOTTOM);
            case "slab_top" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP);
            case "slab_double" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.DOUBLE);
            case "snow_1" -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 1);
            case "snow_7" -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 7);
            case "snow_8" -> Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS, 8);
            case "farmland" -> Blocks.FARMLAND.defaultBlockState();
            case "dirt_path" -> Blocks.DIRT_PATH.defaultBlockState();
            case "stair" -> Blocks.OAK_STAIRS.defaultBlockState().setValue(StairBlock.FACING, Direction.NORTH).setValue(StairBlock.HALF, Half.BOTTOM).setValue(StairBlock.SHAPE, StairsShape.STRAIGHT);
            case "leaves" -> Blocks.OAK_LEAVES.defaultBlockState().setValue(LeavesBlock.DISTANCE, 7).setValue(LeavesBlock.PERSISTENT, false).setValue(LeavesBlock.WATERLOGGED, false);
            default -> throw new IllegalArgumentException(name);
        };
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var world = new SkyColumnOracle();
        if (args.length > 2 && args[2].equals("initial")) {
            var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
            field.setAccessible(true);
            var unsafe = (sun.misc.Unsafe)field.get(null);
            var result = new JsonArray();
            for (var scenario : input) {
                var chunk = (FakeChunk)unsafe.allocateInstance(FakeChunk.class);
                chunk.sections = new LevelChunkSection[2];
                for (int i = 0; i < 2; ++i) {
                    var container = new PalettedContainer<>(Blocks.AIR.defaultBlockState(), Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
                    chunk.sections[i] = new LevelChunkSection(container, null);
                }
                for (var item : scenario.getAsJsonArray()) {
                    var entry = item.getAsJsonObject();
                    int y = entry.get("y").getAsInt();
                    chunk.sections[y >> 4].setBlockState(15, y & 15, 15, state(entry.get("state").getAsString()));
                }
                var sources = new ChunkSkyLightSources(world);
                sources.fillFrom(chunk);
                result.add(sources.getLowestSourceY(15, 15));
            }
            Files.writeString(Path.of(args[1]), result.toString());
            return;
        }
        var sources = new ChunkSkyLightSources(world);
        var result = new JsonArray();
        for (var item : input) {
            var entry = item.getAsJsonObject();
            int y = entry.get("y").getAsInt();
            world.blocks.put(new BlockPos(15, y, 15), state(entry.get("state").getAsString()));
            boolean changed = sources.update(world, 15, y, 15);
            var row = new JsonArray();
            row.add(changed);
            row.add(sources.getLowestSourceY(15, 15));
            result.add(row);
        }
        Files.writeString(Path.of(args[1]), result.toString());
    }
}
