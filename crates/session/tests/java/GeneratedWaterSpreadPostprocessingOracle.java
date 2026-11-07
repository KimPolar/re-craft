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
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.block.state.properties.Half;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.material.*;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.world.level.redstone.*;
import net.minecraft.world.ticks.*;

public class GeneratedWaterSpreadPostprocessingOracle {
    static BlockPos pos = new BlockPos(8,8,8);
    static final Map<BlockPos, BlockState> blocks = new HashMap<>();
    static final List<BlockPos> lightChecks = new ArrayList<>();
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
            if(LightEngine.hasDifferentLightProperties(old,state))lightChecks.add(p.immutable());
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
        try {run(args);} catch(Throwable error){error.printStackTrace(System.err);throw error;}
    }
    static void run(String[] args)throws Exception {
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.WASHED_AWAY_BY_FLUIDS,List.of(Blocks.AIR.builtInRegistryHolder(),Blocks.WATER.builtInRegistryHolder()),
            BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,List.of(),BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP,List.of(),
            BlockTags.FENCES,List.of(Blocks.OAK_FENCE.builtInRegistryHolder()),
            BlockTags.WOODEN_FENCES,List.of(Blocks.OAK_FENCE.builtInRegistryHolder()),BlockTags.SHULKER_BOXES,List.of()))).apply();
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
        var leafTypes = new LinkedHashMap<String,Block>();
        leafTypes.put("oak_leaves",Blocks.OAK_LEAVES); leafTypes.put("spruce_leaves",Blocks.SPRUCE_LEAVES);
        leafTypes.put("birch_leaves",Blocks.BIRCH_LEAVES); leafTypes.put("jungle_leaves",Blocks.JUNGLE_LEAVES);
        leafTypes.put("acacia_leaves",Blocks.ACACIA_LEAVES); leafTypes.put("dark_oak_leaves",Blocks.DARK_OAK_LEAVES);
        leafTypes.put("mangrove_leaves",Blocks.MANGROVE_LEAVES); leafTypes.put("cherry_leaves",Blocks.CHERRY_LEAVES);
        leafTypes.put("pale_oak_leaves",Blocks.PALE_OAK_LEAVES); leafTypes.put("azalea_leaves",Blocks.AZALEA_LEAVES);
        leafTypes.put("flowering_azalea_leaves",Blocks.FLOWERING_AZALEA_LEAVES);
        var scenarios = new ArrayList<>(List.of("down","down_sand","sides","mixed","edge","edge_deferred","convert_off","convert_on","convert_bubble","flow_drain","flow_drain_sand","flow_weaken","flow_falling","flow_down","flow_sides","origin_source_off","origin_source_on","slab_bottom","slab_top","slab_bottom_off","slab_oak","down_wet_slab","marked_slab_dry","marked_slab_wet","marked_slab_side","marked_slab_down","stair_bottom","stair_top","stair_bottom_off","marked_stair_dry","marked_stair_wet","marked_stair_side","marked_stair_down","marked_leaf_dry","marked_leaf_wet","marked_leaf_side","marked_leaf_down"));
        scenarios.addAll(List.of("marked_fence_wet","marked_fence_side","marked_fence_down","marked_fence_disconnect","marked_fence_disconnect_sand","marked_fence_disconnect_water","marked_fence_disconnect_neighbor"));
        scenarios.addAll(List.of("marked_lichen_wet","marked_lichen_side","marked_lichen_down","marked_lichen_sides","marked_lichen_mixed"));
        for(var name:leafTypes.keySet()) scenarios.add("marked_leaf_type_"+name);
        for(var facing:List.of("north","south","west","east"))
            for(var half:List.of("bottom","top"))
                for(var wet:List.of(false,true)) scenarios.add("marked_stair_variant_"+facing+"_"+half+"_"+wet);
        for(var facing:List.of("north","south","west","east"))
            for(var neighbor:List.of("north","south","west","east"))
                for(var half:List.of("bottom","top"))
                    for(var wet:List.of(false,true))
                        for(var side:List.of("front","back")) scenarios.add("marked_stair_corner_"+facing+"_"+neighbor+"_"+half+"_"+wet+"_"+side);
        if(args.length>0 && args[0].equals("lichen"))scenarios.removeIf(s -> !s.startsWith("marked_lichen_"));
        if(args.length>0 && args[0].equals("edge_deferred"))scenarios.removeIf(s -> !s.equals("edge_deferred"));
        if(args.length>0 && args[0].equals("sulfur")){scenarios.clear();scenarios.addAll(List.of("barrier_sulfur","barrier_cinnabar"));}
        for(String scenario:scenarios) {
            bind.invoke(Blocks.DIRT.builtInRegistryHolder(),scenario.equals("convert_bubble")?List.of(BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN):List.of());
            pos=new BlockPos(scenario.startsWith("edge")?15:8,8,8);
            rules.set(GameRules.WATER_SOURCE_CONVERSION,!scenario.equals("convert_off")&&!scenario.equals("origin_source_off")&&!scenario.equals("slab_bottom_off")&&!scenario.equals("stair_bottom_off"),null);
            blocks.clear();lightChecks.clear();blocks.put(pos,Blocks.WATER.defaultBlockState());
            if(scenario.startsWith("barrier_")) {
                var barrier=(scenario.equals("barrier_sulfur")?Blocks.SULFUR:Blocks.CINNABAR).defaultBlockState();
                for(var p:List.of(pos.below(),pos.north(),pos.south(),pos.west()))blocks.put(p,barrier);
                blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.equals("down")||scenario.equals("down_sand")||scenario.equals("down_wet_slab")||scenario.equals("mixed"))blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
            if(scenario.equals("down_sand"))blocks.put(pos.below().east(),Blocks.SAND.defaultBlockState());
            if(scenario.equals("down_wet_slab"))blocks.put(pos.below().east(),Blocks.STONE_SLAB.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,true));
            if(scenario.equals("sides"))for(var p:List.of(pos.north(),pos.south(),pos.west(),pos.east()))blocks.put(p,Blocks.AIR.defaultBlockState());
            if(scenario.equals("mixed")) {
                for(var p:List.of(pos.north(),pos.south(),pos.west()))blocks.put(p,Blocks.WATER.defaultBlockState());
                blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.startsWith("edge"))blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
            if(scenario.equals("edge_deferred")){blocks.put(pos.east(3),Blocks.SAND.defaultBlockState());blocks.put(pos.east(5),Blocks.CHEST.defaultBlockState());}
            if(scenario.equals("convert_off")||scenario.equals("convert_on")||scenario.equals("convert_bubble")) {
                blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                blocks.put(pos.east().north(),Blocks.WATER.defaultBlockState());
                if(scenario.equals("convert_bubble"))blocks.put(pos.east().below(),Blocks.DIRT.defaultBlockState());
            }
            if(scenario.startsWith("slab_")) {
                blocks.put(pos.east(),(scenario.equals("slab_oak")?Blocks.OAK_SLAB:Blocks.STONE_SLAB).defaultBlockState().setValue(BlockStateProperties.SLAB_TYPE,scenario.equals("slab_top")?SlabType.TOP:SlabType.BOTTOM));
                blocks.put(pos.east().north(),Blocks.WATER.defaultBlockState());
            }
            if(scenario.startsWith("marked_slab_")) {
                blocks.put(pos,Blocks.STONE_SLAB.defaultBlockState()
                    .setValue(BlockStateProperties.SLAB_TYPE,scenario.equals("marked_slab_down")?SlabType.TOP:SlabType.BOTTOM)
                    .setValue(BlockStateProperties.WATERLOGGED,!scenario.equals("marked_slab_dry")));
                if(scenario.equals("marked_slab_side"))blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                if(scenario.equals("marked_slab_down"))blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.startsWith("marked_fence_")) {
                blocks.put(pos,Blocks.OAK_FENCE.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,true).setValue(FenceBlock.EAST,scenario.startsWith("marked_fence_disconnect")));
                if(scenario.equals("marked_fence_side")||scenario.startsWith("marked_fence_disconnect"))blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                if(scenario.equals("marked_fence_down"))blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
                if(scenario.equals("marked_fence_disconnect_sand"))blocks.put(pos.north(),Blocks.SAND.defaultBlockState());
                if(scenario.equals("marked_fence_disconnect_water"))blocks.put(pos.north(),Blocks.WATER.defaultBlockState());
                if(scenario.equals("marked_fence_disconnect_neighbor"))blocks.put(pos.north(),Blocks.OAK_FENCE.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,true));
            }
            if(scenario.startsWith("marked_lichen_")) {
                blocks.put(pos,Blocks.GLOW_LICHEN.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,true)
                    .setValue(MultifaceBlock.getFaceProperty(Direction.WEST),true));
                if(scenario.equals("marked_lichen_side") || scenario.equals("marked_lichen_sides") || scenario.equals("marked_lichen_mixed"))blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                if(scenario.equals("marked_lichen_down") || scenario.equals("marked_lichen_mixed"))blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
                if(scenario.equals("marked_lichen_sides") || scenario.equals("marked_lichen_mixed"))for(var p:List.of(pos.north(),pos.south()))blocks.put(p,Blocks.AIR.defaultBlockState());
            }
            if(scenario.startsWith("stair_")) {
                blocks.put(pos.east(),Blocks.OAK_STAIRS.defaultBlockState().setValue(BlockStateProperties.HALF,scenario.equals("stair_top")?Half.TOP:Half.BOTTOM));
                blocks.put(pos.east().north(),Blocks.WATER.defaultBlockState());
            }
            if(scenario.startsWith("marked_stair_corner_")) {
                var parts=scenario.substring("marked_stair_corner_".length()).split("_");
                var facing=Direction.byName(parts[0]);
                var half=parts[2].equals("top")?Half.TOP:Half.BOTTOM;
                blocks.put(pos,Blocks.OAK_STAIRS.defaultBlockState()
                    .setValue(BlockStateProperties.HORIZONTAL_FACING,facing)
                    .setValue(BlockStateProperties.HALF,half)
                    .setValue(BlockStateProperties.WATERLOGGED,Boolean.parseBoolean(parts[3])));
                blocks.put(pos.relative(parts[4].equals("front")?facing:facing.getOpposite()),
                    Blocks.OAK_STAIRS.defaultBlockState()
                        .setValue(BlockStateProperties.HORIZONTAL_FACING,Direction.byName(parts[1]))
                        .setValue(BlockStateProperties.HALF,half));
            } else if(scenario.startsWith("marked_stair_variant_")) {
                var parts=scenario.substring("marked_stair_variant_".length()).split("_");
                blocks.put(pos,Blocks.OAK_STAIRS.defaultBlockState()
                    .setValue(BlockStateProperties.HORIZONTAL_FACING,Direction.byName(parts[0]))
                    .setValue(BlockStateProperties.HALF,parts[1].equals("top")?Half.TOP:Half.BOTTOM)
                    .setValue(BlockStateProperties.WATERLOGGED,Boolean.parseBoolean(parts[2])));
            } else if(scenario.startsWith("marked_stair_")) {
                blocks.put(pos,Blocks.OAK_STAIRS.defaultBlockState()
                    .setValue(BlockStateProperties.HALF,scenario.equals("marked_stair_down")?Half.TOP:Half.BOTTOM)
                    .setValue(BlockStateProperties.WATERLOGGED,!scenario.equals("marked_stair_dry")));
                if(scenario.equals("marked_stair_side"))blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                if(scenario.equals("marked_stair_down"))blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.startsWith("marked_leaf_")) {
                var block=scenario.startsWith("marked_leaf_type_")?leafTypes.get(scenario.substring("marked_leaf_type_".length())):Blocks.OAK_LEAVES;
                blocks.put(pos,block.defaultBlockState()
                    .setValue(LeavesBlock.DISTANCE,7)
                    .setValue(BlockStateProperties.WATERLOGGED,!scenario.equals("marked_leaf_dry")));
                if(scenario.equals("marked_leaf_side"))blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                if(scenario.equals("marked_leaf_down"))blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.equals("flow_drain")||scenario.equals("flow_drain_sand")) {
                blocks.put(pos,Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,1));
                if(scenario.equals("flow_drain_sand"))blocks.put(pos.east(),Blocks.SAND.defaultBlockState());
            }
            if(List.of("flow_weaken","flow_falling","flow_down","flow_sides","origin_source_off","origin_source_on").contains(scenario)) {
                blocks.put(pos,Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,4));
                if(scenario.equals("flow_falling"))blocks.put(pos.above(),Blocks.WATER.defaultBlockState());
                else {
                    blocks.put(pos.west(),Blocks.WATER.defaultBlockState());
                    if(scenario.equals("flow_down"))blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
                    if(scenario.equals("flow_sides"))for(var p:List.of(pos.north(),pos.south(),pos.east()))blocks.put(p,Blocks.AIR.defaultBlockState());
                    if(scenario.startsWith("origin_source_"))blocks.put(pos.north(),Blocks.WATER.defaultBlockState());
                }
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
            for(var p:List.of(pos,pos.below(),pos.north(),pos.south(),pos.west(),pos.east())) {
                var state=get(p);out.printf("state %d,%d,%d %s %s%n",p.getX(),p.getY(),p.getZ(),
                    BuiltInRegistries.BLOCK.getKey(state.getBlock()),state.getBlock()==Blocks.WATER?state.getValue(LiquidBlock.LEVEL).toString():(state.getBlock() instanceof SlabBlock || state.getBlock() instanceof StairBlock || state.getBlock() instanceof LeavesBlock || state.getBlock() instanceof FenceBlock || state.getBlock() instanceof MultifaceBlock)?state.getValue(BlockStateProperties.WATERLOGGED).toString():"-");
                if(state.getBlock() instanceof MultifaceBlock)out.printf("lichen %d,%d,%d %s %s %s %s %s %s%n",p.getX(),p.getY(),p.getZ(),state.getValue(MultifaceBlock.getFaceProperty(Direction.DOWN)),state.getValue(MultifaceBlock.getFaceProperty(Direction.UP)),state.getValue(MultifaceBlock.getFaceProperty(Direction.NORTH)),state.getValue(MultifaceBlock.getFaceProperty(Direction.SOUTH)),state.getValue(MultifaceBlock.getFaceProperty(Direction.WEST)),state.getValue(MultifaceBlock.getFaceProperty(Direction.EAST)));
                if(state.getBlock() instanceof StairBlock)out.printf("stair %d,%d,%d %s %s %s%n",p.getX(),p.getY(),p.getZ(),state.getValue(BlockStateProperties.HORIZONTAL_FACING),state.getValue(BlockStateProperties.HALF),state.getValue(BlockStateProperties.STAIRS_SHAPE));
                if(state.getBlock() instanceof FenceBlock)out.printf("fence %d,%d,%d %s %s %s %s%n",p.getX(),p.getY(),p.getZ(),state.getValue(FenceBlock.NORTH),state.getValue(FenceBlock.EAST),state.getValue(FenceBlock.SOUTH),state.getValue(FenceBlock.WEST));
            }
            if(scenario.startsWith("marked_stair_corner_"))for(var p:lightChecks)out.printf("light %d,%d,%d%n",p.getX(),p.getY(),p.getZ());
            for(var tick:bt.pack(100)){var p=tick.pos();out.printf("block %s %d,%d,%d %d%n",BuiltInRegistries.BLOCK.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            for(var tick:eastBt.pack(100)){var p=tick.pos();out.printf("block %s %d,%d,%d %d%n",BuiltInRegistries.BLOCK.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            for(var tick:ft.pack(100)){var p=tick.pos();out.printf("fluid %s %d,%d,%d %d%n",BuiltInRegistries.FLUID.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            for(var tick:eastFt.pack(100)){var p=tick.pos();out.printf("fluid %s %d,%d,%d %d%n",BuiltInRegistries.FLUID.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            out.println("counter "+counter);
        }
        out.println("END_RESULT");
    }
}
