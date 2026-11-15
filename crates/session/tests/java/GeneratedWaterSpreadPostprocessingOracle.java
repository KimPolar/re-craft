import java.lang.reflect.*;
import java.util.*;
import it.unimi.dsi.fastutil.shorts.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.attribute.EnvironmentAttributeMap;
import net.minecraft.world.attribute.EnvironmentAttributes;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.block.state.properties.Half;
import net.minecraft.world.level.block.state.properties.DoubleBlockHalf;
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
    static final List<BlockPos> fizzEvents = new ArrayList<>();
    static final List<FallingBlockEntity> fallingEntities = new ArrayList<>();
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
        @Override public int getNextEntityId(){return 1;}
        @Override public boolean addFreshEntity(Entity entity){fallingEntities.add((FallingBlockEntity)entity);return true;}
        @Override public GameRules getGameRules(){return rules;}
        @Override public FeatureFlagSet enabledFeatures(){return FeatureFlags.DEFAULT_FLAGS;}
        @Override public EnvironmentAttributeSystem environmentAttributes(){return attributes;}
        @Override public LevelTicks<Block> getBlockTicks(){return blockAccess;}
        @Override public LevelTicks<Fluid> getFluidTicks(){return fluidAccess;}
        @Override public long getGameTime(){return 100;}
        @Override public long nextSubTickCount(){return counter++;}
        @Override public void levelEvent(net.minecraft.world.entity.Entity source,int event,BlockPos p,int data){
            if(event!=1501 || data!=0)throw new AssertionError("unexpected level event "+event);
            fizzEvents.add(p.immutable());
        }
        @Override public void neighborChanged(BlockPos p,Block block,Orientation orientation){updater.neighborChanged(p,block,orientation);}
        @Override public void neighborShapeChanged(Direction dir,BlockPos target,BlockPos source,BlockState state,int flags,int depth){updater.shapeUpdate(dir,state,target,source,flags,depth);}
        // Capture the Level.destroyBlock replacement and neighbor callbacks;
        // loot and item RNG are compared independently by LadderDropLootOracle.
        @Override public boolean destroyBlock(BlockPos p,boolean drops,net.minecraft.world.entity.Entity entity,int depth){
            if(!drops || entity!=null)throw new AssertionError("unexpected destruction flags");
            return setBlock(p,getFluidState(p).createLegacyBlock(),3,depth);
        }
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
        for(boolean drag:new boolean[]{false,true}) {
            var bubble=Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN,drag);
            var water=Blocks.WATER.defaultBlockState();
            if(LightEngine.hasDifferentLightProperties(water,bubble) || water.getLightDampening()!=bubble.getLightDampening() || !bubble.getFluidState().isSource())
                throw new AssertionError("water/bubble light or fluid differs: "+bubble);
        }
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.WASHED_AWAY_BY_FLUIDS,List.of(Blocks.AIR.builtInRegistryHolder(),Blocks.WATER.builtInRegistryHolder()),
            BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,List.of(),BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP,List.of(),
            BlockTags.FENCES,List.of(Blocks.OAK_FENCE.builtInRegistryHolder()),
            BlockTags.WOODEN_FENCES,List.of(Blocks.OAK_FENCE.builtInRegistryHolder()),BlockTags.SHULKER_BOXES,List.of(),
            BlockTags.SPELEOTHEMS,List.of(Blocks.SULFUR_SPIKE.builtInRegistryHolder(),Blocks.POINTED_DRIPSTONE.builtInRegistryHolder())))).apply();
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
        if(args.length>0 && args[0].equals("stalactite_chain")) {
            var tickMethod=SpeleothemBlock.class.getDeclaredMethod("tick",BlockState.class,ServerLevel.class,BlockPos.class,RandomSource.class);tickMethod.setAccessible(true);
            var hurts=FallingBlockEntity.class.getDeclaredField("hurtEntities");hurts.setAccessible(true);
            var amount=FallingBlockEntity.class.getDeclaredField("fallDamagePerDistance");amount.setAccessible(true);
            var top=new BlockPos(8,14,8);
            for(var block:List.of(Blocks.POINTED_DRIPSTONE,Blocks.SULFUR_SPIKE)) for(int length:new int[]{1,2,4,7}) for(boolean wet:new boolean[]{false,true}) {
                blocks.clear();lightChecks.clear();fizzEvents.clear();fallingEntities.clear();
                var bt=new LevelChunkTicks<Block>(List.of());var ft=new LevelChunkTicks<Fluid>(List.of());
                blockAccess=new LevelTicks<Block>(p->true);fluidAccess=new LevelTicks<Fluid>(p->true);
                blockAccess.addContainer(new ChunkPos(0,0),bt);fluidAccess.addContainer(new ChunkPos(0,0),ft);
                counter=0;random=RandomSource.create(42);set(level,"random",random);updater=new CollectingNeighborUpdater(level,100000);
                for(int i=0;i<length;i++) {
                    int remaining=length-i;
                    var thickness=remaining==1?net.minecraft.world.level.block.state.properties.SpeleothemThickness.TIP:
                        remaining==2?net.minecraft.world.level.block.state.properties.SpeleothemThickness.FRUSTUM:
                        remaining==3?net.minecraft.world.level.block.state.properties.SpeleothemThickness.MIDDLE:
                        net.minecraft.world.level.block.state.properties.SpeleothemThickness.BASE;
                    blocks.put(top.below(i),block.defaultBlockState().setValue(SpeleothemBlock.TIP_DIRECTION,Direction.DOWN)
                        .setValue(SpeleothemBlock.THICKNESS,thickness).setValue(SpeleothemBlock.WATERLOGGED,wet));
                }
                tickMethod.invoke(block,get(top),level,top,random);
                out.printf("%s %d %s %d",BuiltInRegistries.BLOCK.getKey(block),length,wet,fallingEntities.size());
                for(int i=0;i<fallingEntities.size();i++) {
                    var entity=fallingEntities.get(i);var state=entity.getBlockState();
                    out.printf(" %d:%s:%s:%s:%s:%s",(int)entity.getY(),state.getValue(SpeleothemBlock.THICKNESS),
                        state.getValue(SpeleothemBlock.WATERLOGGED),hurts.getBoolean(entity),amount.getFloat(entity),
                        BuiltInRegistries.BLOCK.getKey(get(top.below(i)).getBlock()));
                }
                out.println();
            }
            out.println("END_RESULT");return;
        }
        if(args.length>0 && (args[0].equals("speleothem_tick") || args[0].equals("speleothem_tick_untagged"))) {
            boolean untagged=args[0].equals("speleothem_tick_untagged");
            var tickMethod=SpeleothemBlock.class.getDeclaredMethod("tick",BlockState.class,ServerLevel.class,BlockPos.class,RandomSource.class);
            tickMethod.setAccessible(true);
            for(var block:List.of(Blocks.POINTED_DRIPSTONE,Blocks.SULFUR_SPIKE))
                for(var direction:List.of(Direction.UP,Direction.DOWN))
                    for(var wet:List.of(false,true)) for(var restored:List.of(false,true)) {
                        bind.invoke(block.builtInRegistryHolder(),List.of(BlockTags.SPELEOTHEMS));
                        blocks.clear();lightChecks.clear();fizzEvents.clear();fallingEntities.clear();
                        var bt=new LevelChunkTicks<Block>(List.of());var ft=new LevelChunkTicks<Fluid>(List.of());
                        blockAccess=new LevelTicks<Block>(p->true);fluidAccess=new LevelTicks<Fluid>(p->true);
                        blockAccess.addContainer(new ChunkPos(0,0),bt);fluidAccess.addContainer(new ChunkPos(0,0),ft);
                        counter=0;random=RandomSource.create(42);set(level,"random",random);updater=new CollectingNeighborUpdater(level,100000);
                        var spike=block.defaultBlockState().setValue(SpeleothemBlock.TIP_DIRECTION,direction).setValue(SpeleothemBlock.WATERLOGGED,wet);
                        var support=pos.relative(direction.getOpposite());blocks.put(pos,spike);
                        if(!level.setBlock(support,Blocks.AIR.defaultBlockState(),3,512))throw new AssertionError("support unchanged");
                        var scheduled=bt.pack(100);
                        if(scheduled.size()!=1)throw new AssertionError("expected one speleothem tick: "+scheduled);
                        int delay=scheduled.getFirst().delay();
                        if(restored)level.setBlock(support,Blocks.STONE.defaultBlockState(),3,512);
                        if(untagged)bind.invoke(block.builtInRegistryHolder(),List.of());
                        tickMethod.invoke(block,get(pos),level,pos,random);
                        var result=get(pos);
                        out.printf("%s %s %s %s %d %s %d%n",BuiltInRegistries.BLOCK.getKey(block),direction,wet,restored,delay,BuiltInRegistries.BLOCK.getKey(result.getBlock()),fallingEntities.size());
                    }
            out.println("END_RESULT");
            return;
        }
        var leafTypes = new LinkedHashMap<String,Block>();
        leafTypes.put("oak_leaves",Blocks.OAK_LEAVES); leafTypes.put("spruce_leaves",Blocks.SPRUCE_LEAVES);
        leafTypes.put("birch_leaves",Blocks.BIRCH_LEAVES); leafTypes.put("jungle_leaves",Blocks.JUNGLE_LEAVES);
        leafTypes.put("acacia_leaves",Blocks.ACACIA_LEAVES); leafTypes.put("dark_oak_leaves",Blocks.DARK_OAK_LEAVES);
        leafTypes.put("mangrove_leaves",Blocks.MANGROVE_LEAVES); leafTypes.put("cherry_leaves",Blocks.CHERRY_LEAVES);
        leafTypes.put("pale_oak_leaves",Blocks.PALE_OAK_LEAVES); leafTypes.put("azalea_leaves",Blocks.AZALEA_LEAVES);
        leafTypes.put("flowering_azalea_leaves",Blocks.FLOWERING_AZALEA_LEAVES);
        var scenarios = new ArrayList<>(List.of("down","down_sand","sides","mixed","edge","edge_deferred","convert_off","convert_on","convert_bubble","flow_drain","flow_drain_sand","flow_weaken","flow_falling","flow_down","flow_sides","origin_source_off","origin_source_on","slab_bottom","slab_top","slab_bottom_off","slab_oak","down_wet_slab","marked_slab_dry","marked_slab_wet","marked_slab_side","marked_slab_down","stair_bottom","stair_top","stair_bottom_off","marked_stair_dry","marked_stair_wet","marked_stair_side","marked_stair_down","marked_leaf_dry","marked_leaf_wet","marked_leaf_side","marked_leaf_down"));
        scenarios.addAll(List.of("fence_on","fence_off","fence_on_neighbor"));
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
        if(args.length>0 && args[0].equals("lichen_transition")){scenarios.clear();scenarios.add("marked_lichen_lose_east");}
        if(args.length>0 && args[0].equals("lichen_destroy")){scenarios.clear();scenarios.addAll(List.of("marked_lichen_destroy_on","marked_lichen_destroy_off"));}
        if(args.length>0 && args[0].equals("dry_lichen")){scenarios.clear();scenarios.addAll(List.of("dry_lichen_on","dry_lichen_off","dry_lichen_down","dry_lichen_support"));}
        if(args.length>0 && args[0].equals("wet_tip_neighbor")){scenarios.clear();scenarios.addAll(List.of("wet_tip_neighbor_sulfur","wet_tip_neighbor_dripstone","wet_tip_neighbor_sulfur_merge"));}
        if(args.length>0 && args[0].equals("marked_speleothem")){scenarios.clear();scenarios.addAll(List.of("marked_speleothem_sulfur","marked_speleothem_dripstone","marked_speleothem_sulfur_stack","marked_speleothem_sulfur_down","marked_speleothem_sulfur_falling"));}
        if(args.length>0 && args[0].equals("speleothem_water")) {
            scenarios.clear();
            for(var wet:List.of("wet","dry")) for(var material:List.of("sulfur","dripstone"))
                for(var direction:List.of("up","down")) for(var thickness:List.of("base","middle","frustum","tip","merge"))
                    scenarios.add("speleothem_water_"+wet+"_"+material+"_"+direction+"_"+thickness);
        }
        if(args.length>0 && args[0].equals("speleothem_shape")) {
            scenarios.clear();
            for(var wet:List.of("wet","dry")) for(var material:List.of("sulfur","dripstone"))
                scenarios.add("speleothem_shape_"+wet+"_"+material);
        }
        if(args.length>0 && args[0].equals("potent_sulfur_barrier")){scenarios.clear();scenarios.add("barrier_potent_sulfur_wet");}
        if(args.length>0 && args[0].equals("bubble_column")){scenarios.clear();scenarios.addAll(List.of("bubble_magma","bubble_magma_stack","bubble_magma_near","bubble_magma_both","bubble_magma_spike"));}
        if(args.length>0 && args[0].equals("dry_lichen"))for(var state:Blocks.GLOW_LICHEN.getStateDefinition().getPossibleStates())
            if(state.isSolid() || state.canOcclude())throw new AssertionError("lichen has a solid or occluding state: "+state);
        if(args.length>0 && args[0].equals("edge_deferred"))scenarios.removeIf(s -> !s.equals("edge_deferred"));
        if(args.length>0 && args[0].equals("aquatic_noop")) {
            scenarios.clear();
            var kinds=new ArrayList<String>();kinds.add("body");
            for(int age=0;age<=25;age++)kinds.add(Integer.toString(age));
            kinds.addAll(List.of("short","lower","upper"));
            for(var kind:kinds)for(var side:List.of("below","north","south","west","east"))
                scenarios.add("aquatic_noop_"+kind+"_"+side);
            for(var kind:kinds)for(var side:List.of("north","south","west","east"))
                scenarios.add("aquatic_flow_"+kind+"_"+side);
        }
        if(args.length>0 && args[0].equals("seagrass_postprocessing")) {
            scenarios.clear();scenarios.addAll(List.of("seagrass_change","seagrass_neighbor_stable","seagrass_neighbor_tagged","seagrass_neighbor_broken","marked_seagrass_stable","marked_seagrass_side","marked_seagrass_tagged","marked_seagrass_unsupported"));
        }
        if(args.length>0 && args[0].equals("kelp_postprocessing")) {
            scenarios.clear();scenarios.addAll(List.of("kelp_live_side_body","kelp_live_side_head","kelp_live_over_body","kelp_live_over_head","kelp_live_under_body","kelp_live_under_head","kelp_live_marked_body","kelp_live_marked_body_tagged","kelp_live_marked_head","kelp_live_marked_head_tagged","kelp_live_marked_head_chain"));
        }
        if(args.length>0 && args[0].equals("sulfur")){scenarios.clear();scenarios.addAll(List.of("barrier_sulfur","barrier_cinnabar"));}
        if(args.length>0 && args[0].equals("building_barriers")){
            scenarios.clear();for(var name:List.of("oak_planks","deepslate_bricks","light_blue_concrete","white_wool","blue_ice"))scenarios.add("barrier_"+name);
        }
        if(args.length>0 && args[0].equals("lava_drain")){
            scenarios.clear();for(int amount=1;amount<=15;amount++)scenarios.add("lava_drain_"+amount);
        }
        if(args.length>0 && args[0].equals("lava_spread")){
            scenarios.clear();scenarios.addAll(List.of("lava_down_slow","lava_down_fast","lava_sides_slow","lava_sides_fast","lava_rise_slow","lava_rise_fast","lava_convert_off","lava_convert_on"));
        }
        if(args.length>0 && args[0].equals("lava_water_down")){
            scenarios.clear();scenarios.addAll(List.of("lava_water_down_slow","lava_water_down_fast","lava_water_down_flow","lava_water_down_adjacent"));
        }
        if(args.length>0 && args[0].equals("lava_neighbors")){
            scenarios.clear();scenarios.addAll(List.of("lava_touch_water","lava_touch_basalt","lava_touch_basalt_source_tag","lava_touch_basalt_flowing_tag","water_touch_lava_source","water_touch_lava_flow","water_touch_lava_cascade","lava_convert_on_water"));
        }
        if(args.length>0 && args[0].equals("lava_waterlogged")){
            scenarios.clear();scenarios.addAll(List.of("lava_wet_slab_bottom","lava_wet_slab_top","lava_wet_stair","lava_wet_fence","lava_wet_ladder","lava_wet_lichen","lava_wet_leaves"));
        }
        if(args.length>0 && args[0].equals("lava_dry_neighbors")){
            scenarios.clear();scenarios.addAll(List.of("lava_dry_slab_bottom","lava_dry_slab_top","lava_dry_stair","lava_dry_fence","lava_dry_ladder","lava_dry_lichen","lava_dry_leaves","lava_dry_slab_bottom_target","lava_dry_slab_top_target","lava_dry_stair_target","lava_dry_fence_target","lava_dry_ladder_target","lava_dry_lichen_target","lava_dry_leaves_target"));
        }
        if(args.length>0 && args[0].equals("lava_falling")){
            scenarios.clear();for(var block:List.of("sand","red_sand","gravel")){scenarios.add("lava_down_"+block);scenarios.add("lava_drain_"+block);}scenarios.add("lava_drain_sand_all");
        }
        if(args.length>0 && args[0].equals("lava_drain_shapes")){
            scenarios.clear();for(var wet:List.of("dry","wet"))for(var block:List.of("slab_bottom","slab_top","stair","fence","ladder","lichen","leaves"))scenarios.add("lava_"+wet+"_"+block+"_drain");
        }
        if(args.length>0 && args[0].equals("lava_drain_liquids")){
            scenarios.clear();for(var side:List.of("west","east","north","south","below","above"))for(var amount:List.of("source","flow"))for(var suffix:List.of("","_unconfigured"))scenarios.add("lava_drain_liquid_water_"+side+"_"+amount+suffix);
            for(var amount:List.of("source","flow"))scenarios.add("lava_drain_liquid_lava_below_"+amount);
        }
        if(args.length>0 && args[0].equals("down_containers")){
            scenarios.clear();
            for(var kind:List.of("slab_bottom","slab_top","stair_bottom","stair_top","fence"))
                for(var enabled:List.of(true,false))scenarios.add("down_container_"+kind+"_"+enabled);
        }
        if(args.length>0 && args[0].equals("ladder_break")){
            scenarios.clear();scenarios.addAll(List.of("ladder_break_dry_on","ladder_break_dry_off","ladder_break_wet_on","ladder_break_wet_off"));
        }
        if(args.length>0 && args[0].equals("ladder")){
            scenarios.clear();scenarios.addAll(List.of("ladder_on","ladder_off","ladder_down_source","ladder_down_source_off","marked_ladder_dry","marked_ladder_wet","marked_ladder_side","marked_ladder_down","marked_ladder_back","marked_ladder_dry_back"));
            for(var facing:List.of("north","south","west","east")) for(boolean wet:new boolean[]{false,true})
                for(var mode:List.of("sealed","front","back","down"))scenarios.add("marked_ladder_variant_"+facing+"_"+wet+"_"+mode);
        }
        if(args.length>0 && args[0].equals("snow")) {
            scenarios.clear();
            for(var kind:List.of("water","fence")) for(var name:List.of("grass_block","podzol","mycelium"))
                for(var initial:List.of(false,true)) for(var tagged:List.of(false,true))
                    scenarios.add("snow_"+kind+"_"+name+"_"+initial+"_"+tagged);
        }
        for(String scenario:scenarios) {
            bind.invoke(Blocks.STONE.builtInRegistryHolder(),scenario.endsWith("_tagged")?List.of(scenario.startsWith("kelp_live_")?BlockTags.CANNOT_SUPPORT_KELP:BlockTags.CANNOT_SUPPORT_SEAGRASS):List.of());
            bind.invoke(Fluids.LAVA.builtInRegistryHolder(),scenario.equals("lava_touch_basalt_flowing_tag")?List.of():List.of(FluidTags.LAVA));
            bind.invoke(Fluids.FLOWING_LAVA.builtInRegistryHolder(),scenario.equals("lava_touch_basalt_source_tag")?List.of():List.of(FluidTags.LAVA));
            bind.invoke(Blocks.DIRT.builtInRegistryHolder(),scenario.equals("convert_bubble")?List.of(BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN):List.of());
            bind.invoke(Blocks.MAGMA_BLOCK.builtInRegistryHolder(),scenario.equals("bubble_magma_both")?List.of(BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP):scenario.startsWith("bubble_magma")?List.of(BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN):List.of());
            bind.invoke(Blocks.SOUL_SAND.builtInRegistryHolder(),scenario.equals("bubble_soul")?List.of(BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP):List.of());
            boolean snowyWater=scenario.startsWith("snow_water_");
            boolean snowyFence=scenario.startsWith("snow_fence_");
            boolean tagged=scenario.endsWith("_true");
            bind.invoke(Blocks.WATER.builtInRegistryHolder(),snowyWater && tagged?List.of(BlockTags.WASHED_AWAY_BY_FLUIDS,BlockTags.SNOW):List.of(BlockTags.WASHED_AWAY_BY_FLUIDS));
            bind.invoke(Blocks.OAK_FENCE.builtInRegistryHolder(),snowyFence && tagged?List.of(BlockTags.FENCES,BlockTags.WOODEN_FENCES,BlockTags.SNOW):List.of(BlockTags.FENCES,BlockTags.WOODEN_FENCES));
            pos=new BlockPos(scenario.startsWith("edge")?15:8,8,8);
            rules.set(GameRules.WATER_SOURCE_CONVERSION,!scenario.equals("dry_lichen_off")&&!scenario.equals("convert_off")&&!scenario.equals("origin_source_off")&&!scenario.equals("slab_bottom_off")&&!scenario.equals("stair_bottom_off")&&!scenario.equals("fence_off")&&!scenario.equals("ladder_off")&&!scenario.equals("ladder_down_source_off")&&!(scenario.startsWith("down_container_")&&scenario.endsWith("_false")),null);
            rules.set(GameRules.LAVA_SOURCE_CONVERSION,scenario.equals("lava_convert_on")||scenario.equals("lava_convert_on_water"),null);
            blocks.clear();lightChecks.clear();fizzEvents.clear();blocks.put(pos,Blocks.WATER.defaultBlockState());
            if(scenario.startsWith("lava_wet_")||scenario.startsWith("lava_dry_")){
                blocks.put(pos,Blocks.LAVA.defaultBlockState().setValue(LiquidBlock.LEVEL,scenario.endsWith("_drain")?4:0));blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                BlockState wet=switch(scenario.replace("lava_dry_","lava_wet_").replace("_target","").replace("_drain","")){
                    case "lava_wet_slab_bottom" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.BOTTOM).setValue(BlockStateProperties.WATERLOGGED,true);
                    case "lava_wet_slab_top" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.TOP).setValue(BlockStateProperties.WATERLOGGED,true);
                    case "lava_wet_stair" -> Blocks.OAK_STAIRS.defaultBlockState().setValue(BlockStateProperties.HORIZONTAL_FACING,Direction.NORTH).setValue(BlockStateProperties.HALF,Half.BOTTOM).setValue(BlockStateProperties.WATERLOGGED,true);
                    case "lava_wet_fence" -> Blocks.OAK_FENCE.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,true);
                    case "lava_wet_ladder" -> Blocks.LADDER.defaultBlockState().setValue(LadderBlock.FACING,Direction.EAST).setValue(BlockStateProperties.WATERLOGGED,true);
                    case "lava_wet_lichen" -> Blocks.GLOW_LICHEN.defaultBlockState().setValue(MultifaceBlock.getFaceProperty(Direction.EAST),true).setValue(BlockStateProperties.WATERLOGGED,true);
                    case "lava_wet_leaves" -> Blocks.OAK_LEAVES.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,true);
                    default -> throw new AssertionError(scenario);
                };
                if(scenario.startsWith("lava_dry_"))wet=wet.setValue(BlockStateProperties.WATERLOGGED,false);
                if(scenario.endsWith("ladder_drain"))wet=wet.setValue(LadderBlock.FACING,Direction.WEST);
                blocks.put(scenario.endsWith("_target")||scenario.endsWith("_drain")?pos.east():pos.east().north(),wet);
                attributes=EnvironmentAttributeSystem.builder().addConstantLayer(EnvironmentAttributeMap.builder().set(EnvironmentAttributes.FAST_LAVA,false).build()).build();
            }
            if(scenario.startsWith("lava_touch_")){
                blocks.put(pos,Blocks.LAVA.defaultBlockState());blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                if(scenario.equals("lava_touch_water"))blocks.put(pos.east().north(),Blocks.WATER.defaultBlockState());
                else {blocks.put(pos.east().below(),Blocks.SOUL_SOIL.defaultBlockState());blocks.put(pos.east().north(),Blocks.BLUE_ICE.defaultBlockState());}
                attributes=EnvironmentAttributeSystem.builder().addConstantLayer(EnvironmentAttributeMap.builder().set(EnvironmentAttributes.FAST_LAVA,false).build()).build();
            }
            if(scenario.startsWith("water_touch_lava_")){
                blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
                blocks.put(pos.below().east(),Blocks.LAVA.defaultBlockState().setValue(LiquidBlock.LEVEL,scenario.endsWith("flow")?4:0));
                if(scenario.equals("water_touch_lava_cascade"))blocks.put(pos.below().west(),Blocks.LAVA.defaultBlockState());
                attributes=EnvironmentAttributeSystem.builder().addConstantLayer(EnvironmentAttributeMap.builder().set(EnvironmentAttributes.FAST_LAVA,false).build()).build();
            }
            if(scenario.startsWith("lava_water_down_")){
                blocks.put(pos,Blocks.LAVA.defaultBlockState());
                var water=Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,scenario.endsWith("_flow")?4:0);
                blocks.put(pos.below(),water);
                if(scenario.endsWith("_adjacent"))blocks.put(pos.below().east(),water);
                attributes=EnvironmentAttributeSystem.builder().addConstantLayer(EnvironmentAttributeMap.builder().set(EnvironmentAttributes.FAST_LAVA,scenario.endsWith("_fast")).build()).build();
            }
            if(scenario.startsWith("lava_down_") || scenario.startsWith("lava_sides_") || scenario.startsWith("lava_rise_") || scenario.startsWith("lava_convert_")){
                blocks.put(pos,Blocks.LAVA.defaultBlockState());
                if(scenario.startsWith("lava_down_")){
                    blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
                    var falling=switch(scenario.substring("lava_down_".length())){case "sand" -> Blocks.SAND;case "red_sand" -> Blocks.RED_SAND;case "gravel" -> Blocks.GRAVEL;default -> null;};
                    if(falling!=null)blocks.put(pos.below().east(),falling.defaultBlockState());
                }
                else if(scenario.startsWith("lava_sides_"))for(var p:List.of(pos.north(),pos.south(),pos.west(),pos.east()))blocks.put(p,Blocks.AIR.defaultBlockState());
                else {blocks.put(pos,Blocks.LAVA.defaultBlockState().setValue(LiquidBlock.LEVEL,4));blocks.put(pos.west(),Blocks.LAVA.defaultBlockState());}
                if(scenario.startsWith("lava_convert_"))blocks.put(pos.north(),Blocks.LAVA.defaultBlockState());
                if(scenario.equals("lava_convert_on_water"))blocks.put(pos.east(),Blocks.WATER.defaultBlockState());
                attributes=EnvironmentAttributeSystem.builder().addConstantLayer(EnvironmentAttributeMap.builder().set(EnvironmentAttributes.FAST_LAVA,scenario.endsWith("_fast")).build()).build();
            }
            if(scenario.startsWith("lava_drain_")){
                var suffix=scenario.substring("lava_drain_".length());
                var falling=switch(suffix){case "sand", "sand_all" -> Blocks.SAND;case "red_sand" -> Blocks.RED_SAND;case "gravel" -> Blocks.GRAVEL;default -> null;};
                blocks.put(pos,Blocks.LAVA.defaultBlockState().setValue(LiquidBlock.LEVEL,falling==null&&!suffix.startsWith("liquid_")?Integer.parseInt(suffix):4));
                if(suffix.equals("sand_all"))for(var p:List.of(pos.west(),pos.east(),pos.north(),pos.south(),pos.below(),pos.above()))blocks.put(p,falling.defaultBlockState());
                else if(falling!=null)blocks.put(pos.above(),falling.defaultBlockState());
                if(suffix.startsWith("liquid_")){
                    var parts=suffix.split("_");
                    var neighbor=switch(parts[2]){case "west" -> pos.west();case "east" -> pos.east();case "north" -> pos.north();case "south" -> pos.south();case "below" -> pos.below();case "above" -> pos.above();default -> throw new AssertionError(suffix);};
                    var liquid=parts[1].equals("water")?Blocks.WATER:Blocks.LAVA;
                    blocks.put(neighbor,liquid.defaultBlockState().setValue(LiquidBlock.LEVEL,parts[3].equals("flow")?4:0));
                    attributes=EnvironmentAttributeSystem.builder().addConstantLayer(EnvironmentAttributeMap.builder().set(EnvironmentAttributes.FAST_LAVA,false).build()).build();
                }
            }
            if(scenario.startsWith("dry_lichen_")) {
                var target=(scenario.equals("dry_lichen_down") || scenario.equals("dry_lichen_support"))?pos.below():pos.east();
                blocks.put(target,Blocks.GLOW_LICHEN.defaultBlockState().setValue(MultifaceBlock.getFaceProperty(Direction.EAST),true));
                if(scenario.equals("dry_lichen_down")){blocks.put(target.north(),Blocks.WATER.defaultBlockState());blocks.put(target.south(),Blocks.WATER.defaultBlockState());}
                else if(scenario.equals("dry_lichen_support")){blocks.put(pos,Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,4));blocks.put(pos.west(),Blocks.WATER.defaultBlockState());blocks.put(pos.north(),Blocks.WATER.defaultBlockState());}
                else blocks.put(target.north(),Blocks.WATER.defaultBlockState());
            }
            if(scenario.startsWith("wet_tip_neighbor_")) {
                var block=scenario.contains("_sulfur")?Blocks.SULFUR_SPIKE:Blocks.POINTED_DRIPSTONE;
                var spike=block.defaultBlockState().setValue(BlockStateProperties.VERTICAL_DIRECTION,Direction.UP).setValue(BlockStateProperties.WATERLOGGED,true);
                if(scenario.endsWith("_merge"))spike=spike.setValue(SpeleothemBlock.THICKNESS,net.minecraft.world.level.block.state.properties.SpeleothemThickness.TIP_MERGE);
                blocks.put(pos.north(),spike);
                blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.startsWith("marked_speleothem_")) {
                var block=scenario.contains("_sulfur")?Blocks.SULFUR_SPIKE:Blocks.POINTED_DRIPSTONE;
                var down=scenario.endsWith("_down")||scenario.endsWith("_falling");
                var spike=block.defaultBlockState().setValue(BlockStateProperties.VERTICAL_DIRECTION,down?Direction.DOWN:Direction.UP).setValue(BlockStateProperties.WATERLOGGED,!down);
                if(down)spike=spike.setValue(SpeleothemBlock.THICKNESS,net.minecraft.world.level.block.state.properties.SpeleothemThickness.FRUSTUM);
                blocks.put(pos,spike);
                if(down){blocks.put(pos.below(),Blocks.AIR.defaultBlockState());if(scenario.endsWith("_falling"))blocks.put(pos.above(),Blocks.AIR.defaultBlockState());}
                else blocks.put(pos.above(),scenario.endsWith("_stack")?get(pos):Blocks.AIR.defaultBlockState());
            }
            if(scenario.startsWith("speleothem_water_")) {
                var parts=scenario.split("_");
                var block=parts[3].equals("sulfur")?Blocks.SULFUR_SPIKE:Blocks.POINTED_DRIPSTONE;
                var direction=parts[4].equals("up")?Direction.UP:Direction.DOWN;
                var thickness=net.minecraft.world.level.block.state.properties.SpeleothemThickness.valueOf(parts[5].equals("merge")?"TIP_MERGE":parts[5].toUpperCase(Locale.ROOT));
                var spike=block.defaultBlockState().setValue(SpeleothemBlock.TIP_DIRECTION,direction)
                    .setValue(SpeleothemBlock.THICKNESS,thickness).setValue(SpeleothemBlock.WATERLOGGED,parts[2].equals("wet"));
                if(parts[2].equals("wet")){blocks.put(pos,spike);blocks.put(pos.east(),Blocks.AIR.defaultBlockState());}
                else {blocks.put(pos.east(),spike);blocks.put(pos.east().north(),Blocks.WATER.defaultBlockState());}
            }
            if(scenario.startsWith("speleothem_shape_")) {
                var block=scenario.endsWith("_sulfur")?Blocks.SULFUR_SPIKE:Blocks.POINTED_DRIPSTONE;
                var spike=block.defaultBlockState().setValue(SpeleothemBlock.TIP_DIRECTION,Direction.DOWN)
                    .setValue(SpeleothemBlock.THICKNESS,net.minecraft.world.level.block.state.properties.SpeleothemThickness.BASE)
                    .setValue(SpeleothemBlock.WATERLOGGED,scenario.contains("_wet_"));
                blocks.put(pos.east(),Blocks.AIR.defaultBlockState());blocks.put(pos.east().above(),spike);
            }
            if(snowyWater || snowyFence) {
                var parts=scenario.split("_");
                var name=scenario.contains("grass_block")?Blocks.GRASS_BLOCK:scenario.contains("podzol")?Blocks.PODZOL:Blocks.MYCELIUM;
                var initial=Boolean.parseBoolean(parts[parts.length-2]);
                var snowy=name.defaultBlockState().setValue(BlockStateProperties.SNOWY,initial);
                if(snowyWater) {blocks.put(pos.below(),Blocks.AIR.defaultBlockState());blocks.put(pos.below(2),snowy);}
                else {blocks.put(pos,Blocks.OAK_FENCE.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,true).setValue(FenceBlock.EAST,true));blocks.put(pos.east(),Blocks.AIR.defaultBlockState());blocks.put(pos.below(),snowy);}
            }
            if(scenario.startsWith("barrier_")) {
                var barrier=scenario.equals("barrier_potent_sulfur_wet")
                    ? Blocks.POTENT_SULFUR.defaultBlockState().setValue(PotentSulfurBlock.STATE,net.minecraft.world.level.block.state.properties.PotentSulfurState.WET)
                    : BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+scenario.substring("barrier_".length()))).defaultBlockState();
                for(var p:List.of(pos.below(),pos.north(),pos.south(),pos.west()))blocks.put(p,barrier);
                blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.startsWith("bubble_")) {
                blocks.put(pos.below(),scenario.equals("bubble_soul")?Blocks.SOUL_SAND.defaultBlockState():Blocks.MAGMA_BLOCK.defaultBlockState());
                if(scenario.equals("bubble_magma_stack"))blocks.put(pos.above(),Blocks.WATER.defaultBlockState());
                if(scenario.equals("bubble_magma_near"))blocks.put(pos.east(),Blocks.WATER.defaultBlockState());
                if(scenario.equals("bubble_magma_spike"))blocks.put(pos.east(),Blocks.SULFUR_SPIKE.defaultBlockState().setValue(BlockStateProperties.VERTICAL_DIRECTION,Direction.UP).setValue(BlockStateProperties.WATERLOGGED,true));
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
            if(scenario.equals("fence_on")||scenario.equals("fence_off")||scenario.equals("fence_on_neighbor")) {
                blocks.put(pos.east(),Blocks.OAK_FENCE.defaultBlockState());
                if(scenario.equals("fence_on_neighbor"))blocks.put(pos.east(2),Blocks.OAK_FENCE.defaultBlockState());
                blocks.put(pos.east().north(),Blocks.WATER.defaultBlockState());
            }
            if(scenario.equals("ladder_on")||scenario.equals("ladder_off")) {
                blocks.put(pos.east(),Blocks.LADDER.defaultBlockState().setValue(LadderBlock.FACING,Direction.NORTH));
                blocks.put(pos.east().north(),Blocks.WATER.defaultBlockState());
            }
            if(scenario.equals("ladder_down_source")||scenario.equals("ladder_down_source_off")) {
                blocks.put(pos.below(),Blocks.LADDER.defaultBlockState().setValue(LadderBlock.FACING,Direction.NORTH));
                blocks.put(pos.below().west(),Blocks.WATER.defaultBlockState());
                blocks.put(pos.below().east(),Blocks.WATER.defaultBlockState());
            }
            if(scenario.startsWith("ladder_break_")) {
                blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                blocks.put(pos.east(2),Blocks.LADDER.defaultBlockState().setValue(LadderBlock.FACING,Direction.EAST)
                    .setValue(BlockStateProperties.WATERLOGGED,scenario.contains("_wet_")));
            }
            if(scenario.startsWith("down_container_")) {
                var state=scenario.contains("slab_")?Blocks.STONE_SLAB.defaultBlockState().setValue(BlockStateProperties.SLAB_TYPE,scenario.contains("_top_")?SlabType.TOP:SlabType.BOTTOM)
                    :scenario.contains("stair_")?Blocks.OAK_STAIRS.defaultBlockState().setValue(BlockStateProperties.HORIZONTAL_FACING,Direction.NORTH).setValue(BlockStateProperties.HALF,scenario.contains("_top_")?Half.TOP:Half.BOTTOM)
                    :Blocks.OAK_FENCE.defaultBlockState();
                blocks.put(pos.below(),state);
                blocks.put(pos.below().west(),Blocks.WATER.defaultBlockState());
                blocks.put(pos.below().east(),Blocks.WATER.defaultBlockState());
            }
            if(scenario.startsWith("marked_ladder_")) {
                blocks.put(pos,Blocks.LADDER.defaultBlockState().setValue(LadderBlock.FACING,Direction.NORTH)
                    .setValue(LadderBlock.WATERLOGGED,!scenario.equals("marked_ladder_dry")&&!scenario.equals("marked_ladder_dry_back")));
                if(scenario.equals("marked_ladder_side"))blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                if(scenario.equals("marked_ladder_down"))blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
                if(scenario.equals("marked_ladder_back")||scenario.equals("marked_ladder_dry_back"))blocks.put(pos.south(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.startsWith("marked_ladder_variant_")) {
                var parts=scenario.substring("marked_ladder_variant_".length()).split("_");
                var facing=Direction.byName(parts[0]);
                blocks.put(pos,Blocks.LADDER.defaultBlockState().setValue(LadderBlock.FACING,facing).setValue(LadderBlock.WATERLOGGED,Boolean.parseBoolean(parts[1])));
                var open=switch(parts[2]){case "front" -> pos.relative(facing);case "back" -> pos.relative(facing.getOpposite());case "down" -> pos.below();default -> null;};
                if(open!=null)blocks.put(open,Blocks.AIR.defaultBlockState());
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
                boolean destroy=scenario.startsWith("marked_lichen_destroy_");
                blocks.put(pos,Blocks.GLOW_LICHEN.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,true)
                    .setValue(MultifaceBlock.getFaceProperty(Direction.WEST),!destroy)
                    .setValue(MultifaceBlock.getFaceProperty(Direction.EAST),destroy || scenario.equals("marked_lichen_lose_east")));
                if(destroy || scenario.equals("marked_lichen_side") || scenario.equals("marked_lichen_sides") || scenario.equals("marked_lichen_mixed") || scenario.equals("marked_lichen_lose_east"))blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
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
            if(scenario.startsWith("aquatic_noop_") || scenario.startsWith("aquatic_flow_")) {
                boolean flow=scenario.startsWith("aquatic_flow_");
                var parts=scenario.substring((flow?"aquatic_flow_":"aquatic_noop_").length()).split("_");
                var plant=switch(parts[0]){
                    case "body" -> Blocks.KELP_PLANT.defaultBlockState();
                    case "short" -> Blocks.SEAGRASS.defaultBlockState();
                    case "lower" -> Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF,DoubleBlockHalf.LOWER);
                    case "upper" -> Blocks.TALL_SEAGRASS.defaultBlockState().setValue(DoublePlantBlock.HALF,DoubleBlockHalf.UPPER);
                    default -> Blocks.KELP.defaultBlockState().setValue(GrowingPlantHeadBlock.AGE,Integer.parseInt(parts[0]));
                };
                var plantPos=switch(parts[1]){case "below" -> pos.below();case "north" -> pos.north();case "south" -> pos.south();case "west" -> pos.west();case "east" -> pos.east();default -> throw new AssertionError(parts[1]);};
                if(!plant.getFluidState().isSource() || plant.isSolid() || plant.canOcclude() || !plant.getCollisionShape(level,plantPos).isEmpty())throw new AssertionError("unsupported kelp fluid shape: "+plant);
                blocks.put(plantPos,plant);
                if(flow)blocks.put(pos,Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,1));
            }
            if(scenario.equals("seagrass_change")){blocks.put(pos.below(),Blocks.SEAGRASS.defaultBlockState());blocks.put(pos.east(),Blocks.AIR.defaultBlockState());}
            if(scenario.startsWith("seagrass_neighbor_")) {
                blocks.put(pos.below(),Blocks.AIR.defaultBlockState());blocks.put(pos.below().east(),Blocks.SEAGRASS.defaultBlockState());
                if(scenario.endsWith("_broken"))blocks.put(pos.below(2).east(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.startsWith("marked_seagrass_")) {
                blocks.put(pos,Blocks.SEAGRASS.defaultBlockState());
                if(scenario.endsWith("_side"))blocks.put(pos.east(),Blocks.AIR.defaultBlockState());
                if(scenario.endsWith("_unsupported"))blocks.put(pos.below(),Blocks.AIR.defaultBlockState());
            }
            if(scenario.startsWith("kelp_live_")) {
                var plant=scenario.contains("_head")?Blocks.KELP.defaultBlockState().setValue(GrowingPlantHeadBlock.AGE,17):Blocks.KELP_PLANT.defaultBlockState();
                if(scenario.contains("_side_")){blocks.put(pos.below(),Blocks.AIR.defaultBlockState());blocks.put(pos.below().east(),plant);}
                else if(scenario.contains("_over_") || scenario.contains("_under_")){
                    blocks.put(pos.east(),Blocks.AIR.defaultBlockState());blocks.put(scenario.contains("_over_")?pos.east().below():pos.east().above(),plant);
                } else {
                    blocks.put(pos,plant);
                    if(scenario.endsWith("_chain"))blocks.put(pos.above(),Blocks.KELP_PLANT.defaultBlockState());
                }
            }
            ShortList[] marks={new ShortArrayList()};marks[0].add(ProtoChunk.packOffsetCoordinates(pos));
            set(chunk,"postProcessing",marks);set(chunk,"pendingBlockEntities",new HashMap<BlockPos,CompoundTag>());
            var bt=new LevelChunkTicks<Block>(List.of());var ft=new LevelChunkTicks<Fluid>(List.of());
            var eastBt=new LevelChunkTicks<Block>(List.of());var eastFt=new LevelChunkTicks<Fluid>(List.of());
            blockAccess=new LevelTicks<Block>(p->true);fluidAccess=new LevelTicks<Fluid>(p->true);
            blockAccess.addContainer(new ChunkPos(0,0),bt);fluidAccess.addContainer(new ChunkPos(0,0),ft);
            blockAccess.addContainer(new ChunkPos(1,0),eastBt);fluidAccess.addContainer(new ChunkPos(1,0),eastFt);
            counter=2;random=RandomSource.create(42);set(level,"random",random);updater=new CollectingNeighborUpdater(level,100000);
            chunk.postProcessGeneration(level);
            long nextRandom=random.nextLong();
            if(!scenario.startsWith("lava_rise_") && !scenario.startsWith("lava_convert_") && !scenario.startsWith("kelp_live_") && nextRandom!=RandomSource.create(42).nextLong())throw new AssertionError("unexpected random use");
            out.println("case "+scenario);
            for(var p:List.of(pos,pos.below(),pos.north(),pos.south(),pos.west(),pos.east())) {
                var state=get(p);out.printf("state %d,%d,%d %s %s%n",p.getX(),p.getY(),p.getZ(),
                    BuiltInRegistries.BLOCK.getKey(state.getBlock()),(state.getBlock()==Blocks.WATER || state.getBlock()==Blocks.LAVA)?state.getValue(LiquidBlock.LEVEL).toString():(state.getBlock() instanceof SlabBlock || state.getBlock() instanceof StairBlock || state.getBlock() instanceof LeavesBlock || state.getBlock() instanceof FenceBlock || state.getBlock() instanceof LadderBlock || state.getBlock() instanceof MultifaceBlock || state.getBlock()==Blocks.SULFUR_SPIKE || state.getBlock()==Blocks.POINTED_DRIPSTONE)?state.getValue(BlockStateProperties.WATERLOGGED).toString():"-");
                if(state.getBlock() instanceof LadderBlock)out.printf("ladder %d,%d,%d %s%n",p.getX(),p.getY(),p.getZ(),state.getValue(LadderBlock.FACING));
                if(state.getBlock() instanceof SpeleothemBlock)out.printf("speleothem %d,%d,%d %s %s%n",p.getX(),p.getY(),p.getZ(),state.getValue(SpeleothemBlock.TIP_DIRECTION),state.getValue(SpeleothemBlock.THICKNESS));
                if(state.getBlock() instanceof MultifaceBlock)out.printf("lichen %d,%d,%d %s %s %s %s %s %s%n",p.getX(),p.getY(),p.getZ(),state.getValue(MultifaceBlock.getFaceProperty(Direction.DOWN)),state.getValue(MultifaceBlock.getFaceProperty(Direction.UP)),state.getValue(MultifaceBlock.getFaceProperty(Direction.NORTH)),state.getValue(MultifaceBlock.getFaceProperty(Direction.SOUTH)),state.getValue(MultifaceBlock.getFaceProperty(Direction.WEST)),state.getValue(MultifaceBlock.getFaceProperty(Direction.EAST)));
                if(state.getBlock() instanceof StairBlock)out.printf("stair %d,%d,%d %s %s %s%n",p.getX(),p.getY(),p.getZ(),state.getValue(BlockStateProperties.HORIZONTAL_FACING),state.getValue(BlockStateProperties.HALF),state.getValue(BlockStateProperties.STAIRS_SHAPE));
                if(state.getBlock() instanceof FenceBlock)out.printf("fence %d,%d,%d %s %s %s %s%n",p.getX(),p.getY(),p.getZ(),state.getValue(FenceBlock.NORTH),state.getValue(FenceBlock.EAST),state.getValue(FenceBlock.SOUTH),state.getValue(FenceBlock.WEST));
            }
            if(scenario.startsWith("speleothem_shape_")) {
                var spike=get(pos.east().above());
                out.printf("shape_above %s %s %s %s%n",BuiltInRegistries.BLOCK.getKey(spike.getBlock()),
                    spike.getValue(SpeleothemBlock.TIP_DIRECTION),spike.getValue(SpeleothemBlock.THICKNESS),spike.getValue(SpeleothemBlock.WATERLOGGED));
            }
            if(scenario.startsWith("seagrass_neighbor_")) {
                var p=pos.below().east();var s=get(p);
                out.printf("plant %d,%d,%d %s %s%n",p.getX(),p.getY(),p.getZ(),BuiltInRegistries.BLOCK.getKey(s.getBlock()),s.is(Blocks.WATER)?s.getValue(LiquidBlock.LEVEL):"-");
            }
            if(scenario.startsWith("kelp_live_")) {
                var p=scenario.contains("_side_")?pos.below().east():scenario.contains("_over_")?pos.east().below():scenario.contains("_under_")?pos.east().above():pos;
                var s=get(p);out.printf("kelp %d,%d,%d %s %s%n",p.getX(),p.getY(),p.getZ(),BuiltInRegistries.BLOCK.getKey(s.getBlock()),s.is(Blocks.KELP)?s.getValue(GrowingPlantHeadBlock.AGE):"-");
            }
            if(scenario.startsWith("bubble_"))for(int y=8;y<=9;y++){
                var s=get(new BlockPos(8,y,8));
                out.printf("column %d %s %s%n",y,BuiltInRegistries.BLOCK.getKey(s.getBlock()),s.is(Blocks.BUBBLE_COLUMN)?s.getValue(BubbleColumnBlock.DRAG_DOWN):"-");
            }
            if(scenario.startsWith("ladder_break_")){var s=get(pos.east(2));out.printf("destroyed %s %s%n",BuiltInRegistries.BLOCK.getKey(s.getBlock()),s.getBlock()==Blocks.WATER?s.getValue(LiquidBlock.LEVEL).toString():"-");}
            if(snowyWater || snowyFence){var p=snowyWater?pos.below(2):pos.below();var s=get(p);out.printf("snow %d,%d,%d %s %s%n",p.getX(),p.getY(),p.getZ(),BuiltInRegistries.BLOCK.getKey(s.getBlock()),s.getValue(BlockStateProperties.SNOWY));}
            if(scenario.equals("fence_on_neighbor")){var s=get(pos.east(2));out.printf("neighbor %s %s %s %s %s%n",s.getValue(FenceBlock.NORTH),s.getValue(FenceBlock.EAST),s.getValue(FenceBlock.SOUTH),s.getValue(FenceBlock.WEST),s.getValue(BlockStateProperties.WATERLOGGED));}
            if(scenario.startsWith("bubble_")||scenario.startsWith("marked_stair_corner_")||scenario.equals("fence_on_neighbor")||scenario.startsWith("marked_lichen_destroy_")||scenario.startsWith("lava_")||scenario.startsWith("water_touch_lava_"))for(var p:lightChecks)out.printf("light %d,%d,%d%n",p.getX(),p.getY(),p.getZ());
            for(var p:fizzEvents)out.printf("fizz %d,%d,%d%n",p.getX(),p.getY(),p.getZ());
            if(scenario.startsWith("water_touch_lava_"))out.printf("reaction 9,7,8 %s%n",BuiltInRegistries.BLOCK.getKey(get(pos.below().east()).getBlock()));
            if(scenario.equals("water_touch_lava_cascade"))out.printf("reaction 7,7,8 %s%n",BuiltInRegistries.BLOCK.getKey(get(pos.below().west()).getBlock()));
            if(scenario.startsWith("lava_wet_")||scenario.startsWith("lava_dry_")){
                var wetPos=scenario.endsWith("_target")||scenario.endsWith("_drain")?pos.east():pos.east().north();
                var wet=get(wetPos);
                out.printf("wet %d,%d,%d %s %s%n",wetPos.getX(),wetPos.getY(),wetPos.getZ(),BuiltInRegistries.BLOCK.getKey(wet.getBlock()),wet.getValue(BlockStateProperties.WATERLOGGED));
                if(wet.getBlock() instanceof StairBlock)out.printf("wet_stair %s%n",wet.getValue(BlockStateProperties.STAIRS_SHAPE));
                if(wet.getBlock() instanceof FenceBlock)out.printf("wet_fence %s %s %s %s%n",wet.getValue(FenceBlock.NORTH),wet.getValue(FenceBlock.EAST),wet.getValue(FenceBlock.SOUTH),wet.getValue(FenceBlock.WEST));
            }
            for(var tick:bt.pack(100)){var p=tick.pos();out.printf("block %s %d,%d,%d %d%n",BuiltInRegistries.BLOCK.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            for(var tick:eastBt.pack(100)){var p=tick.pos();out.printf("block %s %d,%d,%d %d%n",BuiltInRegistries.BLOCK.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            for(var tick:ft.pack(100)){var p=tick.pos();out.printf("fluid %s %d,%d,%d %d%n",BuiltInRegistries.FLUID.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            for(var tick:eastFt.pack(100)){var p=tick.pos();out.printf("fluid %s %d,%d,%d %d%n",BuiltInRegistries.FLUID.getKey(tick.type()),p.getX(),p.getY(),p.getZ(),tick.delay());}
            out.println("counter "+counter);
            if(scenario.startsWith("lava_rise_") || scenario.startsWith("lava_convert_") || scenario.startsWith("kelp_live_"))out.println("random "+nextRandom);
        }
        out.println("END_RESULT");
    }
}
