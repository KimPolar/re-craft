import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.redstone.CollectingNeighborUpdater;

public class SpeleothemWaterTickOracle {
    static final BlockPos CENTER = new BlockPos(8, 10, 8);
    static class World extends IceBubbleTagOracle.World {
        GameRules rules;
        World() { super(); }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.STONE.defaultBlockState()); }
        @Override public GameRules getGameRules() { return rules; }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 32; }
        @Override public boolean setBlockAndUpdate(BlockPos pos, BlockState state) { return setBlock(pos, state, 3, 512); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(
            FluidTags.WATER, List.of(Fluids.WATER.builtInRegistryHolder(), Fluids.FLOWING_WATER.builtInRegistryHolder()),
            FluidTags.BUBBLE_COLUMN_CAN_OCCUPY, List.of(Fluids.WATER.builtInRegistryHolder())))).apply();
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.WASHED_AWAY_BY_FLUIDS, List.of(Blocks.AIR.builtInRegistryHolder(), Blocks.WATER.builtInRegistryHolder()),
            BlockTags.SPELEOTHEMS, List.of(Blocks.POINTED_DRIPSTONE.builtInRegistryHolder(), Blocks.SULFUR_SPIKE.builtInRegistryHolder()),
            BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN, List.of(),
            BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP, List.of()))).apply();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        world.attributes = EnvironmentAttributeSystem.builder().build();
        world.rules = new GameRules(FeatureFlags.DEFAULT_FLAGS);
        var rows = new JsonArray();
        for (var spike : List.of(Blocks.POINTED_DRIPSTONE, Blocks.SULFUR_SPIKE)) {
            for (var thickness : List.of(SpeleothemThickness.TIP, SpeleothemThickness.TIP_MERGE)) {
                world.blocks = new HashMap<>(); world.random = new LegacyRandomSource(1);
                world.updater = new CollectingNeighborUpdater(world, 100000);
                world.blockTicks = new JsonArray(); world.fluidTicks = new JsonArray(); world.pending = new HashSet<>();
                var initial = spike.defaultBlockState().setValue(SpeleothemBlock.TIP_DIRECTION, Direction.UP)
                    .setValue(SpeleothemBlock.THICKNESS, thickness).setValue(SpeleothemBlock.WATERLOGGED, true);
                world.blocks.put(CENTER, initial); world.blocks.put(CENTER.east(), Blocks.AIR.defaultBlockState());
                Fluids.WATER.tick(world, CENTER, initial, initial.getFluidState());
                var east = world.getBlockState(CENTER.east());
                var row = new JsonObject(); row.addProperty("block", BuiltInRegistries.BLOCK.getKey(spike).toString());
                row.addProperty("thickness", thickness.getSerializedName());
                row.addProperty("east", BuiltInRegistries.BLOCK.getKey(east.getBlock()).toString());
                row.addProperty("level", east.is(Blocks.WATER) ? east.getValue(LiquidBlock.LEVEL) : -1);
                row.addProperty("center_unchanged", world.getBlockState(CENTER) == initial);
                row.add("fluid_ticks", world.fluidTicks); rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
