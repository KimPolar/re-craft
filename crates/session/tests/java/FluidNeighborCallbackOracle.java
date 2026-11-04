import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.material.*;
import net.minecraft.world.level.redstone.CollectingNeighborUpdater;

public class FluidNeighborCallbackOracle {
    static class World extends BubbleTickCallbackOracle.World {
        GameRules rules;
        @Override public GameRules getGameRules() { return rules; }
        @Override public void levelEvent(Entity entity, int id, BlockPos position, int data) { }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(
            FluidTags.WATER, List.of(Fluids.WATER.builtInRegistryHolder(), Fluids.FLOWING_WATER.builtInRegistryHolder()),
            FluidTags.LAVA, List.of(Fluids.LAVA.builtInRegistryHolder(), Fluids.FLOWING_LAVA.builtInRegistryHolder())
        ))).apply();
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.WASHED_AWAY_BY_FLUIDS, List.of(Blocks.AIR.builtInRegistryHolder(), Blocks.WATER.builtInRegistryHolder(), Blocks.LAVA.builtInRegistryHolder())
        ))).apply();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world = (World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
        world.attributes = EnvironmentAttributeSystem.builder().build();
        world.rules = new GameRules(FeatureFlags.DEFAULT_FLAGS);
        world.blocks = new HashMap<>(); world.random = new LegacyRandomSource(1);
        world.blockTicks = new JsonArray(); world.fluidTicks = new JsonArray(); world.pending = new HashSet<>();
        world.updater = new CollectingNeighborUpdater(world, 100000);
        var center = new BlockPos(8,5,8);
        for (var pos : List.of(center, center.north(), center.south(), center.west()))
            world.blocks.put(pos, Blocks.LAVA.defaultBlockState());
        world.blocks.put(center.above(), Blocks.WATER.defaultBlockState());
        world.blocks.put(center.below(), Blocks.WATER.defaultBlockState());
        var original = world.getBlockState(center);
        ((FlowingFluid)original.getFluidState().getType()).tick(world, center, original, original.getFluidState());
        var states = new JsonArray();
        for (var pos : List.of(center, center.below(), center.east()))
            states.add(BuiltInRegistries.BLOCK.getKey(world.getBlockState(pos).getBlock()).toString());
        Files.writeString(Path.of(args[0]), states.toString());
    }
}
