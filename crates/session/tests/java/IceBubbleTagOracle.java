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
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.redstone.*;
import net.minecraft.world.phys.Vec3;

public class IceBubbleTagOracle {
    static final BlockPos CENTER = new BlockPos(8, 10, 8);
    static class World extends ServerLevel {
        Map<BlockPos, BlockState> blocks; RandomSource random; boolean bubble; JsonArray blockTicks, fluidTicks; Set<String> pending;
        CollectingNeighborUpdater updater; EnvironmentAttributeSystem attributes;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public RandomSource getRandom() { return random; }
        @Override public int getBrightness(LightLayer layer, BlockPos pos) { return 15; }
        @Override public int getMaxLocalRawBrightness(BlockPos pos) { return 15; }
        @Override public ResourceKey<Level> dimension() { return Level.OVERWORLD; }
        @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.DEFAULT_FLAGS; }
        @Override public EnvironmentAttributeSystem environmentAttributes() { return attributes; }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState()); }
        @Override public net.minecraft.world.level.material.FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
        @Override public void neighborChanged(BlockPos pos, Block block, Orientation orientation) { updater.neighborChanged(pos, block, orientation); }
        @Override public void neighborShapeChanged(Direction dir, BlockPos target, BlockPos source, BlockState state, int flags, int depth) { updater.shapeUpdate(dir, state, target, source, flags, depth); }
        @Override public boolean setBlock(BlockPos pos, BlockState state, int flags, int depth) {
            var old = getBlockState(pos); if (old == state) return false; blocks.put(pos.immutable(), state);
            if ((flags & 512) == 0) state.onPlace(this, pos, old, false);
            if ((flags & 1) != 0) updater.updateNeighborsAtExceptFromFacing(pos, old.getBlock(), null, null);
            if ((flags & 16) == 0 && depth > 0) { int next = flags & -34; old.updateIndirectNeighbourShapes(this, pos, next, depth-1); state.updateNeighbourShapes(this, pos, next, depth-1); state.updateIndirectNeighbourShapes(this, pos, next, depth-1); }
            return true;
        }
        @Override public void scheduleTick(BlockPos pos, Block block, int delay) {
            if (block == Blocks.WATER) bubble = true;
            String id=block==Blocks.WATER?"water":block==Blocks.FROSTED_ICE?"ice":block==Blocks.BUBBLE_COLUMN?"bubble":"other";
            if(!pending.add("b:"+pos.toShortString()+":"+id)) return;
            var entry=new JsonArray(); entry.add(id); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); blockTicks.add(entry);
        }
        @Override public void scheduleTick(BlockPos pos, net.minecraft.world.level.material.Fluid fluid, int delay) {
            String id=fluid==Fluids.WATER?"water":"flowing_water";
            if(!pending.add("f:"+pos.toShortString()+":"+id)) return;
            var entry=new JsonArray(); entry.add(id); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); fluidTicks.add(entry);
        }
        @Override public void gameEvent(Holder<GameEvent> event, Vec3 pos, GameEvent.Context context) { }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        world.attributes=EnvironmentAttributeSystem.builder().build();
        var tick=FrostedIceBlock.class.getDeclaredMethod("tick",BlockState.class,ServerLevel.class,BlockPos.class,RandomSource.class); tick.setAccessible(true);
        var rows=new JsonArray();
        for (int scenario=0; scenario<3; scenario++) for (boolean supportTagged:new boolean[]{false,true}) for (boolean waterTagged:new boolean[]{false,true}) for (boolean source:new boolean[]{false,true}) {
            var support = scenario==2 ? Blocks.FROSTED_ICE : Blocks.STONE;
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
                BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN, supportTagged ? List.of(support.builtInRegistryHolder()) : List.of(),
                BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP, List.of()))).apply();
            BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(
                FluidTags.WATER, List.of(Fluids.WATER.builtInRegistryHolder(), Fluids.FLOWING_WATER.builtInRegistryHolder()),
                FluidTags.BUBBLE_COLUMN_CAN_OCCUPY, waterTagged ? List.of(Fluids.WATER.builtInRegistryHolder()) : List.of()))).apply();
            world.blocks=new HashMap<>(); world.random=new LegacyRandomSource(1); world.bubble=false; world.updater=new CollectingNeighborUpdater(world, 100000); world.blockTicks=new JsonArray(); world.fluidTicks=new JsonArray(); world.pending=new HashSet<>();
            int age=scenario==2 ? 0 : 3;
            var ice=Blocks.FROSTED_ICE.defaultBlockState().setValue(FrostedIceBlock.AGE,age);
            world.blocks.put(CENTER,ice);
            if (scenario==0) world.blocks.put(CENTER.below(),Blocks.STONE.defaultBlockState());
            if (scenario==1) { world.blocks.put(CENTER.east().below(),Blocks.STONE.defaultBlockState()); world.blocks.put(CENTER.east(),Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,source?0:1)); }
            if (scenario==2) world.blocks.put(CENTER.above(),Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,source?0:1));
            tick.invoke(Blocks.FROSTED_ICE,ice,world,CENTER,world.random);
            var row=new JsonObject(); row.addProperty("scenario",scenario); row.addProperty("support",supportTagged); row.addProperty("water",waterTagged); row.addProperty("source",source); row.addProperty("bubble",world.bubble); row.addProperty("final",world.getBlockState(CENTER).toString()); row.addProperty("block_tag",support.defaultBlockState().is(BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN)); row.addProperty("fluid_tag",Fluids.WATER.defaultFluidState().is(FluidTags.BUBBLE_COLUMN_CAN_OCCUPY)); row.add("blockTicks",world.blockTicks); row.add("fluidTicks",world.fluidTicks); row.addProperty("next",world.random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
