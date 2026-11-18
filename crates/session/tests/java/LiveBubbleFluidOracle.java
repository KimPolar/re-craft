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
import net.minecraft.world.entity.Entity;
import net.minecraft.world.flag.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.material.*;
import net.minecraft.world.level.redstone.*;
import net.minecraft.world.phys.Vec3;

/** Tick source water with real neighbor callbacks and a bounded synthetic world. */
public class LiveBubbleFluidOracle {
    static final BlockPos CENTER = new BlockPos(8, 10, 8);
    static class World extends ServerLevel {
        Map<BlockPos, BlockState> blocks;
        RandomSource random;
        CollectingNeighborUpdater updater;
        EnvironmentAttributeSystem attributes;
        GameRules rules;
        JsonArray changes, fluidTicks, blockTicks, calls;
        Set<String> pending;
        int gameEvents, levelEvents;
        boolean destroyed;
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public RandomSource getRandom() { return random; }
        @Override public ResourceKey<Level> dimension() { return Level.OVERWORLD; }
        @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.DEFAULT_FLAGS; }
        @Override public EnvironmentAttributeSystem environmentAttributes() { return attributes; }
        @Override public GameRules getGameRules() { return rules; }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 32; }
        @Override public BlockState getBlockState(BlockPos p) { return blocks.getOrDefault(p, Blocks.STONE.defaultBlockState()); }
        @Override public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
        @Override public boolean destroyBlock(BlockPos p, boolean drops, Entity entity, int depth) {
            destroyed = true;
            return setBlock(p, getFluidState(p).createLegacyBlock(), 3, depth);
        }
        @Override public void neighborChanged(BlockPos p, Block block, Orientation orientation) { updater.neighborChanged(p, block, orientation); }
        @Override public void neighborShapeChanged(Direction direction, BlockPos target, BlockPos source, BlockState state, int flags, int depth) { updater.shapeUpdate(direction, state, target, source, flags, depth); }
        @Override public boolean setBlock(BlockPos p, BlockState state, int flags, int depth) {
            var old = getBlockState(p);
            if (old == state) return false;
            blocks.put(p.immutable(), state);
            var row = new JsonArray(); row.add(p.getX()); row.add(p.getY()); row.add(p.getZ());
            row.add(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
            row.add(state.is(Blocks.WATER) ? state.getValue(LiquidBlock.LEVEL) : -1);
            row.add(state.is(Blocks.KELP) ? state.getValue(GrowingPlantHeadBlock.AGE) : -1);
            changes.add(row);
            if ((flags & 512) == 0) state.onPlace(this, p, old, false);
            if ((flags & 1) != 0) updater.updateNeighborsAtExceptFromFacing(p, old.getBlock(), null, null);
            if ((flags & 16) == 0 && depth > 0) {
                int next = flags & -34;
                old.updateIndirectNeighbourShapes(this, p, next, depth - 1);
                state.updateNeighbourShapes(this, p, next, depth - 1);
                state.updateIndirectNeighbourShapes(this, p, next, depth - 1);
            }
            return true;
        }
        void scheduled(String kind, String id, BlockPos p, int delay, JsonArray ticks) {
            var row = new JsonArray(); row.add(kind); row.add(id); row.add(p.getX()); row.add(p.getY()); row.add(p.getZ()); row.add(delay);
            calls.add(row);
            if (pending.add(kind + ":" + id + ":" + p.toShortString())) ticks.add(row);
        }
        @Override public void scheduleTick(BlockPos p, Block block, int delay) { scheduled("block", BuiltInRegistries.BLOCK.getKey(block).toString(), p, delay, blockTicks); }
        @Override public void scheduleTick(BlockPos p, Fluid fluid, int delay) { scheduled("fluid", BuiltInRegistries.FLUID.getKey(fluid).toString(), p, delay, fluidTicks); }
        @Override public void gameEvent(Holder<GameEvent> event, Vec3 p, GameEvent.Context context) { gameEvents++; }
        @Override public void levelEvent(Entity entity, int id, BlockPos p, int data) { levelEvents++; }
    }
    static BlockState wall(boolean mossy, boolean complex) {
        var state = (mossy ? Blocks.MOSSY_STONE_BRICK_WALL : Blocks.STONE_BRICK_WALL).defaultBlockState()
            .setValue(WallBlock.WATERLOGGED, true);
        return complex ? state.setValue(WallBlock.SOUTH, WallSide.TALL) : state;
    }
    static BlockState variant(int index) {
        return switch (index) {
            case 0 -> Blocks.STONE.defaultBlockState();
            case 1 -> Blocks.MOSSY_STONE_BRICKS.defaultBlockState();
            case 2 -> Blocks.CRACKED_STONE_BRICKS.defaultBlockState();
            case 3 -> Blocks.CRYING_OBSIDIAN.defaultBlockState();
            case 4 -> Blocks.GRAVEL.defaultBlockState();
            case 5 -> Blocks.SUSPICIOUS_GRAVEL.defaultBlockState().setValue(BlockStateProperties.DUSTED, 2);
            case 6 -> Blocks.CHEST.defaultBlockState().setValue(ChestBlock.WATERLOGGED, true);
            case 7 -> Blocks.CHEST.defaultBlockState().setValue(ChestBlock.WATERLOGGED, true).setValue(ChestBlock.TYPE, ChestType.LEFT);
            case 8 -> Blocks.STONE_BRICK_STAIRS.defaultBlockState().setValue(StairBlock.FACING, Direction.EAST).setValue(StairBlock.WATERLOGGED, true);
            case 9 -> wall(false, false);
            case 10 -> wall(true, true);
            case 11 -> Blocks.KELP.defaultBlockState().setValue(GrowingPlantHeadBlock.AGE, 17);
            case 12 -> Blocks.KELP_PLANT.defaultBlockState();
            case 14 -> Blocks.SEAGRASS.defaultBlockState();
            case 15 -> Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF, DoubleBlockHalf.LOWER);
            case 16 -> Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF, DoubleBlockHalf.UPPER);
            case 17 -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.WATERLOGGED, true);
            default -> Blocks.GLOWSTONE.defaultBlockState();
        };
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.WALLS, List.of(Blocks.STONE_BRICK_WALL.builtInRegistryHolder(), Blocks.MOSSY_STONE_BRICK_WALL.builtInRegistryHolder()),
            BlockTags.WALL_POST_OVERRIDE, List.of(), BlockTags.SHULKER_BOXES, List.of(),
            BlockTags.CANNOT_SUPPORT_KELP, List.of(), BlockTags.CANNOT_SUPPORT_SEAGRASS, List.of(), BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN, List.of(),
            BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP, List.of(),
            BlockTags.WASHED_AWAY_BY_FLUIDS, List.of(Blocks.AIR.builtInRegistryHolder(), Blocks.WATER.builtInRegistryHolder(), Blocks.LAVA.builtInRegistryHolder())))).apply();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(
            FluidTags.WATER, List.of(Fluids.WATER.builtInRegistryHolder(), Fluids.FLOWING_WATER.builtInRegistryHolder()),
            FluidTags.LAVA, List.of(Fluids.LAVA.builtInRegistryHolder(), Fluids.FLOWING_LAVA.builtInRegistryHolder()),
            FluidTags.BUBBLE_COLUMN_CAN_OCCUPY, List.of(Fluids.WATER.builtInRegistryHolder())))).apply();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world = (World) ((sun.misc.Unsafe) field.get(null)).allocateInstance(World.class);
        world.attributes = EnvironmentAttributeSystem.builder().build();
        world.rules = new GameRules(FeatureFlags.DEFAULT_FLAGS);
        var rows = new JsonArray();
        for (int source = 0; source < 6; source++) for (int mode = 0; mode < 3; mode++)
        for (int variant = 0; variant < 18; variant++) for (var direction : Direction.values()) {
            var target = mode == 1 ? CENTER.below() : CENTER.east();
            if (mode == 0 && direction != Direction.EAST) continue;
            if (mode != 0 && target.relative(direction).equals(CENTER)) continue;
            world.blocks = new HashMap<>(); world.random = RandomSource.create(42);
            world.updater = new CollectingNeighborUpdater(world, 100000);
            world.changes = new JsonArray(); world.fluidTicks = new JsonArray(); world.blockTicks = new JsonArray();
            world.calls = new JsonArray(); world.pending = new HashSet<>(); world.gameEvents = 0; world.levelEvents = 0; world.destroyed = false;
            var root = switch (source) {
                case 0 -> Blocks.BUBBLE_COLUMN.defaultBlockState();
                case 1 -> Blocks.KELP.defaultBlockState().setValue(GrowingPlantHeadBlock.AGE, 7);
                case 2 -> wall(false, false);
                case 3 -> Blocks.SEAGRASS.defaultBlockState();
                case 4 -> Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF, DoubleBlockHalf.LOWER);
                default -> Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF, DoubleBlockHalf.UPPER);
            };
            world.blocks.put(CENTER, root);
            var special = mode == 0 ? CENTER.east() : target.relative(direction);
            world.blocks.put(special, variant(variant));
            if (mode != 0) world.blocks.put(target, Blocks.AIR.defaultBlockState());
            Fluids.WATER.tick(world, CENTER, root, root.getFluidState());
            var row = new JsonObject(); row.addProperty("source", source); row.addProperty("mode", mode);
            row.addProperty("variant", variant); row.addProperty("direction", direction.getName());
            row.add("changes", world.changes); row.add("fluidTicks", world.fluidTicks); row.add("blockTicks", world.blockTicks);
            row.add("calls", world.calls); row.addProperty("gameEvents", world.gameEvents); row.addProperty("levelEvents", world.levelEvents); row.addProperty("next", world.random.nextInt());
            row.addProperty("destroyed", world.destroyed);
            rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
