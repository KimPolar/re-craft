import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.resources.ResourceKey;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.flag.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.material.*;
import net.minecraft.world.level.redstone.*;
import net.minecraft.world.phys.Vec3;

public class KelpGrowthOracle {
    static final BlockPos CENTER = new BlockPos(8, 10, 8);
    static final Direction[] SIDES = {Direction.WEST, Direction.EAST, Direction.NORTH, Direction.SOUTH};
    static class World extends ServerLevel {
        Map<BlockPos, BlockState> blocks;
        RandomSource random;
        CollectingNeighborUpdater updater;
        EnvironmentAttributeSystem attributes;
        JsonArray changes, blockTicks, fluidTicks, fluidCalls;
        Set<String> pending;
        int gameEvents;
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public RandomSource getRandom() { return random; }
        @Override public ResourceKey<Level> dimension() { return Level.OVERWORLD; }
        @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.DEFAULT_FLAGS; }
        @Override public EnvironmentAttributeSystem environmentAttributes() { return attributes; }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.STONE.defaultBlockState()); }
        @Override public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
        @Override public void neighborChanged(BlockPos pos, Block block, Orientation orientation) { updater.neighborChanged(pos, block, orientation); }
        @Override public void neighborShapeChanged(Direction dir, BlockPos target, BlockPos source, BlockState state, int flags, int depth) { updater.shapeUpdate(dir, state, target, source, flags, depth); }
        @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int depth) {
            var old = getBlockState(pos);
            if (old == state) return false;
            blocks.put(pos.immutable(), state);
            var entry = new JsonArray();
            entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ());
            entry.add(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
            entry.add(state.is(Blocks.KELP) ? state.getValue(GrowingPlantHeadBlock.AGE) : -1);
            entry.add(flags); changes.add(entry);
            if ((flags & 512) == 0) state.onPlace(this, pos, old, false);
            if ((flags & 1) != 0) updater.updateNeighborsAtExceptFromFacing(pos, old.getBlock(), null, null);
            if ((flags & 16) == 0 && depth > 0) {
                int next = flags & -34;
                old.updateIndirectNeighbourShapes(this, pos, next, depth - 1);
                state.updateNeighbourShapes(this, pos, next, depth - 1);
                state.updateIndirectNeighbourShapes(this, pos, next, depth - 1);
            }
            return true;
        }
        @Override public void scheduleTick(BlockPos pos, Block block, int delay) {
            String id = BuiltInRegistries.BLOCK.getKey(block).toString();
            if (!pending.add("b:" + pos.toShortString() + ":" + id)) return;
            var entry = new JsonArray(); entry.add(id); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); blockTicks.add(entry);
        }
        @Override public void scheduleTick(BlockPos pos, Fluid fluid, int delay) {
            String id = BuiltInRegistries.FLUID.getKey(fluid).toString();
            var entry = new JsonArray(); entry.add(id); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); fluidTicks.add(entry);
            fluidCalls.add(entry);
            if (!pending.add("f:" + pos.toShortString() + ":" + id)) fluidTicks.remove(fluidTicks.size() - 1);
        }
        @Override public void gameEvent(Holder<GameEvent> event, Vec3 pos, GameEvent.Context context) { gameEvents++; }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.CANNOT_SUPPORT_KELP, List.of(),
            BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN, List.of(),
            BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP, List.of()))).apply();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(
            FluidTags.WATER, List.of(Fluids.WATER.builtInRegistryHolder(), Fluids.FLOWING_WATER.builtInRegistryHolder()),
            FluidTags.BUBBLE_COLUMN_CAN_OCCUPY, List.of(Fluids.WATER.builtInRegistryHolder())))).apply();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        world.attributes = EnvironmentAttributeSystem.builder().build();
        var rows = new JsonArray();
        for (int age : new int[] {0, 7, 24, 25})
        for (int target : new int[] {0, 1, 2})
        for (int sides : new int[] {0, 1, 2, 3, 4, 5})
        for (int support : new int[] {0, 1, 2, 3})
        for (int ceiling : new int[] {0, 1, 2, 3})
        for (long sample = 0; sample < 16; sample++) {
            long seed = sample * 1_000_003L - 17L;
            world.blocks = new HashMap<>();
            world.random = new LegacyRandomSource(seed);
            world.updater = new CollectingNeighborUpdater(world, 100000);
            world.changes = new JsonArray(); world.blockTicks = new JsonArray();
            world.fluidTicks = new JsonArray(); world.fluidCalls = new JsonArray(); world.pending = new HashSet<>(); world.gameEvents = 0;
            var head = Blocks.KELP.defaultBlockState().setValue(GrowingPlantHeadBlock.AGE, age);
            world.blocks.put(CENTER, head);
            world.blocks.put(CENTER.below(), switch (support) {
                case 0 -> Blocks.STONE.defaultBlockState();
                case 1 -> Blocks.KELP_PLANT.defaultBlockState();
                case 2 -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 1);
                default -> Blocks.AIR.defaultBlockState();
            });
            world.blocks.put(CENTER.above(), target == 0 ? Blocks.WATER.defaultBlockState()
                : target == 1 ? Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 1)
                : Blocks.STONE.defaultBlockState());
            world.blocks.put(CENTER.above(2), switch (ceiling) {
                case 0 -> Blocks.STONE.defaultBlockState();
                case 1 -> Blocks.WATER.defaultBlockState();
                case 2 -> Blocks.AIR.defaultBlockState();
                default -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 1);
            });
            for (int index = 0; index < SIDES.length; index++) {
                var side = SIDES[index];
                var neighbor = switch (sides) {
                    case 0 -> Blocks.STONE.defaultBlockState();
                    case 1 -> Blocks.WATER.defaultBlockState();
                    case 2 -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 1);
                    case 3 -> Blocks.KELP.defaultBlockState().setValue(GrowingPlantHeadBlock.AGE, 17);
                    case 4 -> Blocks.KELP_PLANT.defaultBlockState();
                    default -> index % 2 == 0 ? Blocks.STONE.defaultBlockState() : Blocks.WATER.defaultBlockState();
                };
                world.blocks.put(CENTER.relative(side), neighbor);
                world.blocks.put(CENTER.above().relative(side), neighbor);
            }
            head.randomTick(world, CENTER, world.random);
            var row = new JsonObject();
            row.addProperty("seed", seed); row.addProperty("age", age);
            row.addProperty("target", target); row.addProperty("sides", sides);
            row.addProperty("support", support); row.addProperty("ceiling", ceiling);
            row.add("changes", world.changes); row.add("blockTicks", world.blockTicks);
            row.add("fluidTicks", world.fluidTicks); row.add("fluidCalls", world.fluidCalls); row.addProperty("gameEvents", world.gameEvents);
            row.addProperty("next", world.random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
