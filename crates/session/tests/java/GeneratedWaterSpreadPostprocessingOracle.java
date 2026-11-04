import java.lang.reflect.*;
import java.util.*;
import it.unimi.dsi.fastutil.shorts.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.material.*;
import net.minecraft.world.level.redstone.*;
import net.minecraft.world.ticks.*;

public class GeneratedWaterSpreadPostprocessingOracle {
    static BlockPos pos = new BlockPos(8,8,8);
    static final Map<BlockPos, BlockState> blocks = new HashMap<>();
    static LevelTicks<Block> blockAccess;
    static LevelTicks<Fluid> fluidAccess;
    static long counter;
    static RandomSource random;
    static CollectingNeighborUpdater updater;
    static GameRules rules;
    static EnvironmentAttributeSystem attributes;
    static void set(Object object,String name,Object value)throws Exception {
        for(Class<?> type=object.getClass();type!=null;type=type.getSuperclass()) {
            try {var field=type.getDeclaredField(name);field.setAccessible(true);field.set(object,value);return;}
            catch(NoSuchFieldException ignored) {}
        }
        throw new NoSuchFieldException(name);
    }
    static BlockState get(BlockPos p) {return blocks.getOrDefault(p,Blocks.STONE.defaultBlockState());}
    static class FakeLevel extends ServerLevel {
        FakeLevel(){super(null,null,null,null,null,null,false,0L,List.of(),false);}
        @Override public BlockState getBlockState(BlockPos p){return get(p);}
        @Override public FluidState getFluidState(BlockPos p){return get(p).getFluidState();}
        @Override public RandomSource getRandom(){return random;}
        @Override public GameRules getGameRules(){return rules;}
        @Override public FeatureFlagSet enabledFeatures(){return FeatureFlags.DEFAULT_FLAGS;}
        @Override public EnvironmentAttributeSystem environmentAttributes(){return attributes;}
        @Override public LevelTicks<Block> getBlockTicks(){return blockAccess;}
        @Override public LevelTicks<Fluid> getFluidTicks(){return fluidAccess;}
        @Override public long getGameTime(){return 100;}
        @Override public long nextSubTickCount(){return counter++;}
        @Override public void neighborChanged(BlockPos p,Block block,Orientation orientation){updater.neighborChanged(p,block,orientation);}
        @Override public void neighborShapeChanged(Direction dir,BlockPos target,BlockPos source,BlockState state,int flags,int depth){updater.shapeUpdate(dir,state,target,source,flags,depth);}
        @Override public boolean setBlock(BlockPos p,BlockState state,int flags,int depth){
            var old=get(p);if(old==state)return false;blocks.put(p.immutable(),state);
            if((flags&512)==0)state.onPlace(this,p,old,false);
            if((flags&1)!=0)updater.updateNeighborsAtExceptFromFacing(p,old.getBlock(),null,null);
            if((flags&16)==0 && depth>0){int next=flags&-34;old.updateIndirectNeighbourShapes(this,p,next,depth-1);state.updateNeighbourShapes(this,p,next,depth-1);state.updateIndirectNeighbourShapes(this,p,next,depth-1);}
            return true;
        }
    }
    static class FakeChunk extends LevelChunk {
        FakeChunk(){super((Level)null,new ChunkPos(0,0));}
        @Override public ChunkPos getPos(){return new ChunkPos(0,0);}
        @Override public int getMinY(){return 0;}
        @Override public int getHeight(){return 16;}
        @Override public BlockState getBlockState(BlockPos p){return get(p);}
    }
    static class FakeUpgrade extends UpgradeData {
        FakeUpgrade(){super(new CompoundTag(),LevelHeightAccessor.create(0,16));}
        @Override public void upgrade(LevelChunk chunk){}
    }
    public static void main(String[] args)throws Exception {
        try {run();} catch(Throwable error){error.printStackTrace(System.err);throw error;}
    }
    static void run()throws Exception {
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.WASHED_AWAY_BY_FLUIDS,List.of(Blocks.AIR.builtInRegistryHolder(),Blocks.WATER.builtInRegistryHolder()),
            BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,List.of(),BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP,List.of()))).apply();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(
            FluidTags.WATER,List.of(Fluids.WATER.builtInRegistryHolder(),Fluids.FLOWING_WATER.builtInRegistryHolder()),
            FluidTags.LAVA,List.of(Fluids.LAVA.builtInRegistryHolder(),Fluids.FLOWING_LAVA.builtInRegistryHolder()),
            FluidTags.BUBBLE_COLUMN_CAN_OCCUPY,List.of(Fluids.WATER.builtInRegistryHolder())))).apply();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
        var level=(FakeLevel)unsafe.allocateInstance(FakeLevel.class);
        var chunk=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
        set(chunk,"upgradeData",unsafe.allocateInstance(FakeUpgrade.class));
        attributes=EnvironmentAttributeSystem.builder().build();rules=new GameRules(FeatureFlags.DEFAULT_FLAGS);
        out.println("BEGIN_RESULT");
        for(String scenario:List.of("down","down_sand","sides","mixed","edge","convert_off","convert_on","convert_bubble")) {
            bind.invoke(Blocks.DIRT.builtInRegistryHolder(),scenario.equals("convert_bubble")?List.of(BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN):List.of());
            pos=new BlockPos(scenario.equals("edge")?15:8,8,8);
            rules.set(GameRules.WATER_SOURCE_CONVERSION,!scenario.equals("convert_off"),null);
            blocks.clear();blocks.put(pos,Blocks.WATER.defaultBlockState());
            if(scenario.equals("down")||scenario.equals("down_sand")||scenario.equals("mixed"))blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
            if(scenario.equals("down_sand"))blocks.put(pos.below().east(),Blocks.SAND.defaultBlockState());
            if(scenario.equals("sides"))for(var p:List.of(pos.north(),pos.south(),pos.west(),pos.east()))blocks.put(p,Blocks.AIR.defaultBlockState());
            if(scenario.equals("mixed")) {
                for(var p:List.of(pos.north(),pos.south(),pos.west()))blocks.put(p,Blocks.WATER.defaultBlockState());
                blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.equals("edge"))blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
            if(scenario.equals("convert_off")||scenario.equals("convert_on")||scenario.equals("convert_bubble")) {
                blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                blocks.put(pos.east().north(),Blocks.WATER.defaultBlockState());
                if(scenario.equals("convert_bubble"))blocks.put(pos.east().below(),Blocks.DIRT.defaultBlockState());
            }
            ShortList[] marks={new ShortArrayList()};marks[0].add(ProtoChunk.packOffsetCoordinates(pos));
            set(chunk,"postProcessing",marks);set(chunk,"pendingBlockEntities",new HashMap<BlockPos,CompoundTag>());
            var bt=new LevelChunkTicks<Block>(List.of());var ft=new LevelChunkTicks<Fluid>(List.of());
            var eastBt=new LevelChunkTicks<Block>(List.of());var eastFt=new LevelChunkTicks<Fluid>(List.of());
            blockAccess=new LevelTicks<Block>(p->true);fluidAccess=new LevelTicks<Fluid>(p->true);
            blockAccess.addContainer(new ChunkPos(0,0),bt);fluidAccess.addContainer(new ChunkPos(0,0),ft);
            blockAccess.addContainer(new ChunkPos(1,0),eastBt);fluidAccess.addContainer(new ChunkPos(1,0),eastFt);
            counter=2;random=RandomSource.create(42);updater=new CollectingNeighborUpdater(level,100000);
            chunk.postProcessGeneration(level);
            if(random.nextLong()!=RandomSource.create(42).nextLong())throw new AssertionError("unexpected random use");
            out.println("case "+scenario);
            for(var p:List.of(pos.below(),pos.north(),pos.south(),pos.west(),pos.east())) {
                var state=get(p);out.printf("state %d,%d,%d %s %s%n",p.getX(),p.getY(),p.getZ(),
                    BuiltInRegistries.BLOCK.getKey(state.getBlock()),state.getBlock()==Blocks.WATER?state.getValue(LiquidBlock.LEVEL).toString():"-");
            }
            for(var tick:bt.pack(100)){var p=tick.pos();out.printf("block %s %d,%d,%d %d%n",BuiltInRegistries.BLOCK.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            for(var tick:eastBt.pack(100)){var p=tick.pos();out.printf("block %s %d,%d,%d %d%n",BuiltInRegistries.BLOCK.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            for(var tick:ft.pack(100)){var p=tick.pos();out.printf("fluid %s %d,%d,%d %d%n",BuiltInRegistries.FLUID.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            for(var tick:eastFt.pack(100)){var p=tick.pos();out.printf("fluid %s %d,%d,%d %d%n",BuiltInRegistries.FLUID.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            out.println("counter "+counter);
        }
        out.println("END_RESULT");
    }
}
