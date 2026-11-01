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

public class BubbleTickCallbackOracle {
    static final BlockPos CENTER = new BlockPos(8,10,8);
    static BlockState cell(int id) { return switch (id) {
        case 0 -> Blocks.AIR.defaultBlockState();
        case 1 -> Blocks.WATER.defaultBlockState();
        case 2 -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,1);
        case 3 -> Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN,true);
        case 4 -> Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN,false);
        case 5 -> Blocks.STONE.defaultBlockState();
        default -> throw new AssertionError();
    }; }
    static String name(BlockState state) {
        if(state.is(Blocks.WATER)) return "water/"+state.getValue(LiquidBlock.LEVEL);
        if(state.is(Blocks.BUBBLE_COLUMN)) return "bubble/"+state.getValue(BubbleColumnBlock.DRAG_DOWN);
        return state.is(Blocks.STONE)?"stone":"air";
    }
    static class World extends ServerLevel {
        Map<BlockPos,BlockState> blocks; RandomSource random; CollectingNeighborUpdater updater;
        EnvironmentAttributeSystem attributes; JsonArray blockTicks, fluidTicks; Set<String> pending;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public RandomSource getRandom() { return random; }
        @Override public ResourceKey<Level> dimension() { return Level.OVERWORLD; }
        @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.DEFAULT_FLAGS; }
        @Override public EnvironmentAttributeSystem environmentAttributes() { return attributes; }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState()); }
        @Override public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
        @Override public void neighborChanged(BlockPos pos,Block block,Orientation orientation) { updater.neighborChanged(pos,block,orientation); }
        @Override public void neighborShapeChanged(Direction dir,BlockPos target,BlockPos source,BlockState state,int flags,int depth) { updater.shapeUpdate(dir,state,target,source,flags,depth); }
        @Override public boolean setBlock(BlockPos pos,BlockState state,int flags,int depth) {
            var old=getBlockState(pos); if(old==state) return false; blocks.put(pos.immutable(),state);
            if((flags & 512)==0) state.onPlace(this,pos,old,false);
            if((flags & 1)!=0) updater.updateNeighborsAtExceptFromFacing(pos,old.getBlock(),null,null);
            if((flags & 16)==0 && depth>0) { int next=flags & -34; old.updateIndirectNeighbourShapes(this,pos,next,depth-1); state.updateNeighbourShapes(this,pos,next,depth-1); state.updateIndirectNeighbourShapes(this,pos,next,depth-1); }
            return true;
        }
        @Override public void scheduleTick(BlockPos pos,Block block,int delay) {
            String id=block==Blocks.WATER?"water":block==Blocks.BUBBLE_COLUMN?"bubble":"other";
            if(!pending.add("b:"+pos.toShortString()+":"+id)) return;
            var entry=new JsonArray(); entry.add(id); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); blockTicks.add(entry);
        }
        @Override public void scheduleTick(BlockPos pos,Fluid fluid,int delay) {
            String id=fluid==Fluids.WATER?"water":"flowing_water";
            if(!pending.add("f:"+pos.toShortString()+":"+id)) return;
            var entry=new JsonArray(); entry.add(id); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); fluidTicks.add(entry);
        }
        @Override public void gameEvent(Holder<GameEvent> event,Vec3 pos,GameEvent.Context context) { }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class); world.attributes=EnvironmentAttributeSystem.builder().build();
        var waterTick=LiquidBlock.class.getDeclaredMethod("tick",BlockState.class,ServerLevel.class,BlockPos.class,RandomSource.class); waterTick.setAccessible(true);
        var bubbleTick=BubbleColumnBlock.class.getDeclaredMethod("tick",BlockState.class,ServerLevel.class,BlockPos.class,RandomSource.class); bubbleTick.setAccessible(true);
        var rows=new JsonArray();
        for(int support=0;support<4;support++) for(boolean waterTagged:new boolean[]{false,true}) {
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
                BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN, (support==1||support==3)?List.of(Blocks.STONE.builtInRegistryHolder()):List.of(),
                BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP, (support==2||support==3)?List.of(Blocks.STONE.builtInRegistryHolder()):List.of()))).apply();
            BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(
                FluidTags.WATER,List.of(Fluids.WATER.builtInRegistryHolder(),Fluids.FLOWING_WATER.builtInRegistryHolder()),
                FluidTags.BUBBLE_COLUMN_CAN_OCCUPY,waterTagged?List.of(Fluids.WATER.builtInRegistryHolder()):List.of()))).apply();
            for(int root:new int[]{1,3,4}) for(int above=0;above<6;above++) for(int top=0;top<6;top++) for(int side=0;side<5;side++) {
                world.blocks=new HashMap<>(); world.random=new LegacyRandomSource(1); world.updater=new CollectingNeighborUpdater(world,100000);
                world.blockTicks=new JsonArray(); world.fluidTicks=new JsonArray(); world.pending=new HashSet<>();
                world.blocks.put(CENTER.below(),cell(5)); world.blocks.put(CENTER,cell(root)); world.blocks.put(CENTER.above(),cell(above)); world.blocks.put(CENTER.above(2),cell(top));
                if(side!=0) { world.blocks.put(CENTER.east(),cell(side)); world.blocks.put(CENTER.east().below(),cell(5)); }
                (root==1?waterTick:bubbleTick).invoke(root==1?Blocks.WATER:Blocks.BUBBLE_COLUMN,cell(root),world,CENTER,world.random);
                var states=new JsonArray(); for(int i=0;i<3;i++) states.add(name(world.getBlockState(CENTER.above(i))));
                var row=new JsonObject(); row.addProperty("support",support); row.addProperty("water",waterTagged); row.addProperty("root",root); row.addProperty("above",above); row.addProperty("top",top); row.addProperty("side",side);
                row.add("states",states); row.add("blockTicks",world.blockTicks); row.add("fluidTicks",world.fluidTicks); rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
