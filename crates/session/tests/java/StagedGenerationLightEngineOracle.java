import java.nio.file.*;
import java.util.*;
import java.util.function.BiConsumer;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.lighting.*;
import net.minecraft.world.level.material.FluidState;

public class StagedGenerationLightEngineOracle implements LightChunkGetter, BlockGetter {
    final Map<ChunkPos, TestChunk> chunks = new HashMap<>();
    final Map<BlockPos, BlockState> edits = new HashMap<>();
    boolean mixed;
    boolean magmaOnly;
    boolean wide;
    boolean lateFar;
    boolean emptyEast;
    boolean emptyWest;
    boolean sparseEast;
    boolean sparseColumn;
    boolean sparseShadow;
    boolean sparseMany;
    boolean blockCase;
    boolean stairCase;
    boolean stairSky;
    boolean tallSky;
    public int getMinY(){ return 0; }
    public int getHeight(){ return tallSky ? 64 : 16; }
    public BlockGetter getLevel(){ return this; }
    public LightChunk getChunkForLighting(int x,int z){ return chunks.get(new ChunkPos(x,z)); }
    public BlockEntity getBlockEntity(BlockPos p){ return null; }
    public BlockState getBlockState(BlockPos p){
        if(edits.containsKey(p)) return edits.get(p);
        if(blockCase && p.getY()==10 && p.getZ()==8 && (p.getX()==12 || p.getX()==14)) return Blocks.TORCH.defaultBlockState();
        if(sparseEast && ((sparseShadow && p.getY()==11 && ((p.getX()==16 && p.getZ()==8) || (sparseMany && ((p.getX()==16 && p.getZ()==9) || (p.getX()==17 && p.getZ()==8))))) || (!sparseShadow && p.getY()==0 && ((sparseColumn && p.getX()==16 && p.getZ()==8) || (!sparseColumn && p.getX()==31 && p.getZ()==0))))) return Blocks.STONE.defaultBlockState();
        if(emptyEast && Math.floorDiv(p.getX(),16)==1 && Math.floorDiv(p.getZ(),16)==0) return Blocks.AIR.defaultBlockState();
        if(emptyWest && Math.floorDiv(p.getX(),16)==-1 && Math.floorDiv(p.getZ(),16)==0) return Blocks.AIR.defaultBlockState();
        if(mixed && p.getX()==0 && p.getY()==10 && p.getZ()==8) return Blocks.TORCH.defaultBlockState();
        if(lateFar && p.getX()==31 && p.getY()==10 && p.getZ()==8) return Blocks.TORCH.defaultBlockState();
        if(magmaOnly && p.getX()==16 && p.getY()==8 && p.getZ()==8) return Blocks.WATER.defaultBlockState();
        if(!blockCase && !magmaOnly && p.getX()==17 && p.getY()==10 && p.getZ()==8) return Blocks.TORCH.defaultBlockState();
        if(p.getY()>=0 && p.getY()<8 && Math.abs(Math.floorDiv(p.getX(),16))<=((mixed || wide)?2:1) && Math.abs(Math.floorDiv(p.getZ(),16))<=((mixed || wide)?2:1)) return Blocks.STONE.defaultBlockState();
        return Blocks.AIR.defaultBlockState();
    }
    public FluidState getFluidState(BlockPos p){ return getBlockState(p).getFluidState(); }
    class TestChunk implements LightChunk {
        final int x,z;
        final ChunkSkyLightSources sources = new ChunkSkyLightSources(StagedGenerationLightEngineOracle.this);
        TestChunk(int x,int z){ this.x=x; this.z=z; }
        public BlockState getBlockState(BlockPos p){ return StagedGenerationLightEngineOracle.this.getBlockState(new BlockPos(x*16+Math.floorMod(p.getX(),16),p.getY(),z*16+Math.floorMod(p.getZ(),16))); }
        public FluidState getFluidState(BlockPos p){ return getBlockState(p).getFluidState(); }
        public BlockEntity getBlockEntity(BlockPos p){ return null; }
        public int getMinY(){ return 0; }
        public int getHeight(){ return 16; }
        public ChunkSkyLightSources getSkyLightSources(){ return sources; }
        public void findBlockLightSources(BiConsumer<BlockPos,BlockState> f){
            if(x==1 && z==0 && !blockCase && !magmaOnly && !emptyEast) f.accept(new BlockPos(17,10,8),Blocks.TORCH.defaultBlockState());
            if(x==0 && z==0 && blockCase && !stairSky){ f.accept(new BlockPos(12,10,8),Blocks.TORCH.defaultBlockState()); if(!stairCase)f.accept(new BlockPos(14,10,8),Blocks.TORCH.defaultBlockState()); }
            if(x==1 && z==0 && blockCase) for(var entry:edits.entrySet())
                if(Math.floorDiv(entry.getKey().getX(),16)==1 && Math.floorDiv(entry.getKey().getZ(),16)==0 && entry.getValue().getLightEmission()>0) f.accept(entry.getKey(),entry.getValue());
            if(x==1 && z==0 && lateFar) f.accept(new BlockPos(31,10,8),Blocks.TORCH.defaultBlockState());
            if(x==2 && z==0 && lateFar) f.accept(new BlockPos(33,10,8),Blocks.TORCH.defaultBlockState());
            if(x==1 && z==0 && edits.get(new BlockPos(16,7,8))==Blocks.MAGMA_BLOCK.defaultBlockState()) f.accept(new BlockPos(16,7,8),Blocks.MAGMA_BLOCK.defaultBlockState());
            var vine=edits.get(new BlockPos(16,10,8)); if(x==1 && z==0 && vine!=null && vine.getLightEmission()>0) f.accept(new BlockPos(16,10,8),vine);
            if(mixed && x==0 && z==0) f.accept(new BlockPos(0,10,8),Blocks.TORCH.defaultBlockState());
        }
    }
    static class FakeChunk extends ProtoChunk {
        LevelChunkSection section;
        FakeChunk(){ super(null,null,null,null,null); }
        @Override public int getMinY(){ return 0; }
        @Override public int getHeight(){ return 16; }
        @Override public LevelChunkSection getSection(int i){ return section; }
        @Override public int getHighestFilledSectionIndex(){ return 0; }
    }
    static void drain(LevelLightEngine e){ while(e.hasLightWork()) e.runLightUpdates(); }
    static JsonArray capture(LevelLightEngine e){ return capture(e,0,0); }
    static JsonArray capture(LevelLightEngine e,int chunkX){ return capture(e,chunkX,0); }
    static JsonArray capture(LevelLightEngine e,int chunkX,int chunkZ){
        var result=new JsonArray();
        for(var layer:new LightLayer[]{LightLayer.BLOCK,LightLayer.SKY}){
            var values=new JsonArray(); var listener=e.getLayerListener(layer);
            for(int y=-16;y<32;y++) for(int z=0;z<16;z++) for(int x=0;x<16;x++) values.add(listener.getLightValue(new BlockPos(chunkX*16+x,y,chunkZ*16+z)));
            result.add(values);
        }
        return result;
    }
    static JsonArray captureNearby(LevelLightEngine e,boolean all){
        if(!all)return capture(e);
        var result=new JsonArray();
        for(int z=-1;z<=1;z++)for(int x=0;x<=2;x++)result.add(capture(e,x,z));
        return result;
    }
    static JsonArray captureWide(LevelLightEngine e){
        var result=new JsonArray();
        for(int z=-2;z<=2;z++)for(int x=-2;x<=2;x++)result.add(capture(e,x,z));
        return result;
    }
    static JsonArray pendingSkyChecks(LevelLightEngine e)throws Exception{
        var engineField=LevelLightEngine.class.getDeclaredField("skyEngine"); engineField.setAccessible(true);
        var nodesField=LightEngine.class.getDeclaredField("blockNodesToCheck"); nodesField.setAccessible(true);
        var nodes=(it.unimi.dsi.fastutil.longs.LongOpenHashSet)nodesField.get(engineField.get(e));
        var order=new JsonArray(); var iterator=nodes.iterator();
        while(iterator.hasNext()){
            var p=BlockPos.of(iterator.nextLong()); var point=new JsonArray();
            point.add(p.getX()); point.add(p.getY()); point.add(p.getZ()); order.add(point);
        }
        return order;
    }
    static JsonArray captureMixed(LevelLightEngine e,boolean far){
        var result=new JsonArray(); result.add(capture(e,0)); result.add(capture(e,far?1:-1)); result.add(capture(e,far?-1:0,far?0:-1)); return result;
    }
    public static void main(String[] args)throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var w=new StagedGenerationLightEngineOracle();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        boolean vineNoSky=args.length>1 && args[1].equals("tree_empty_vine_no_sky");
        boolean noSky=args.length>1 && (args[1].equals("tree_empty_no_sky") || vineNoSky);
        var engine=new LevelLightEngine(w,true,!noSky);
        boolean patchFreshWater=args.length>1 && args[1].equals("patch_fresh_clay_water");
        boolean freshWaterDripleaf=args.length>1 && args[1].equals("column_fresh_water_dripleaf");
        boolean freshWaterlogDripleaf=args.length>1 && args[1].equals("patch_fresh_waterlog_dripleaf");
        boolean freshDripleafSequence=args.length>1 && args[1].equals("patch_fresh_dripleaf_sequence");
        boolean oreActive=args.length>1 && args[1].equals("ore_active");
        boolean ore=args.length>1 && (args[1].equals("ore") || oreActive);
        boolean leafMany=args.length>1 && args[1].equals("leaf_active_many");
        boolean blockCase=args.length>1 && args[1].startsWith("block_incremental_");
        w.blockCase=blockCase;
        w.stairCase=blockCase && args[1].startsWith("block_incremental_stair_");
        w.stairSky=w.stairCase && args[1].contains("_sky_");
        boolean skyMany=args.length>1 && (args[1].equals("sky_inactive_many") || args[1].equals("sky_inactive_activate_many"));
        boolean skyActivate=args.length>1 && args[1].startsWith("sky_inactive_activate");
        w.tallSky=args.length>1 && args[1].equals("sky_inactive_activate_tall");
        boolean skyDark=args.length>1 && (args[1].equals("sky_inactive_dark") || args[1].equals("sky_inactive_activate_dark"));
        boolean skyShadow=args.length>1 && (args[1].equals("sky_inactive_shadow") || skyMany || (skyActivate && !skyDark));
        boolean vineActive=args.length>1 && args[1].startsWith("vine_active_");
        boolean vineSelf=vineActive && args[1].startsWith("vine_active_self_");
        boolean lichenActive=vineActive && args[1].endsWith("_lichen");
        boolean vineActiveEmpty=vineActive && args[1].contains("_empty_");
        boolean vineActiveMany=vineActive && (args[1].equals("vine_active_many") || args[1].equals("vine_active_self_many"));
        boolean vineActiveTransition=vineActive && args[1].endsWith("_plain_then_glowing");
        boolean stagedActivate=args.length>1 && args[1].equals("tree_empty_full_activate");
        boolean activeEmptyWide=args.length>1 && args[1].equals("tree_empty_active_wide");
        boolean emptyFullWide=args.length>1 && (args[1].equals("tree_empty_full_wide") || stagedActivate || activeEmptyWide);
        boolean registerAfter=args.length>1 && args[1].startsWith("tree_empty_partial_register");
        w.emptyWest=registerAfter && args[1].endsWith("_empty");
        boolean emptyPartialWide=args.length>1 && (args[1].equals("tree_empty_partial_wide") || registerAfter);
        boolean skyWide=skyShadow || skyDark || emptyFullWide || emptyPartialWide || (args.length>1 && (args[1].equals("log_active_sparse_wide") || args[1].equals("log_active_sparse_litter_wide")));
        boolean treeSparse=args.length>1 && (args[1].startsWith("log_active_sparse") || skyShadow || skyDark);
        boolean treeEmptyFull=args.length>1 && (args[1].equals("tree_empty_full") || emptyFullWide || emptyPartialWide);
        boolean treeFreshEmpty=args.length>1 && args[1].equals("tree_fresh_empty");
        boolean emptyLeaf=args.length>1 && args[1].equals("tree_empty_leaf_active");
        boolean treeEmpty=args.length>1 && (args[1].equals("tree_empty") || emptyLeaf || noSky || treeEmptyFull || treeFreshEmpty);
        w.emptyEast=treeEmpty || treeSparse || vineActiveEmpty;
        w.sparseEast=treeSparse;
        w.sparseColumn=args.length>1 && args[1].equals("log_active_sparse_column");
        w.sparseShadow=skyShadow;
        w.sparseMany=skyMany;
        boolean logActive=args.length>1 && args[1].equals("log_active_full");
        boolean leafFull=args.length>1 && (args[1].equals("leaf_active_full") || leafMany || logActive || treeEmptyFull || treeSparse || treeFreshEmpty);
        boolean leafActive=args.length>1 && (args[1].equals("leaf_active") || leafFull || treeEmpty);
        boolean normal=args.length>1 && args[1].equals("normal");
        boolean queued=args.length>1 && (args[1].equals("magma_queued") || args[1].equals("magma_exposed_queued") || args[1].equals("vine_queued") || args[1].equals("vine_plain_queued") || args[1].equals("vine_existing_queued") || args[1].equals("vine_full_queued"));
        boolean magma=args.length>1 && (args[1].equals("magma") || args[1].equals("magma_queued") || args[1].equals("magma_exposed") || args[1].equals("magma_exposed_queued"));
        boolean exposed=args.length>1 && args[1].startsWith("magma_exposed");
        boolean vine=args.length>1 && (vineActive || args[1].equals("vine") || args[1].equals("vine_queued") || args[1].equals("vine_plain") || args[1].equals("vine_plain_queued") || args[1].equals("vine_existing") || args[1].equals("vine_existing_queued") || args[1].equals("vine_full") || args[1].equals("vine_full_queued"));
        boolean full=args.length>1 && args[1].startsWith("vine_full");
        w.wide=full || blockCase || skyActivate || stagedActivate || activeEmptyWide || vineSelf;
        boolean plain=args.length>1 && (args[1].startsWith("vine_plain") || (vineActive && args[1].endsWith("_plain")));
        boolean existing=args.length>1 && (args[1].startsWith("vine_existing") || args[1].equals("vine_active_existing") || args[1].equals("vine_active_self_existing"));
        w.magmaOnly=magma;
        boolean partial=args.length>1 && (args[1].equals("partial") || ore || normal || magma || (vine && !vineActive) || emptyPartialWide || (leafActive && !leafFull));
        boolean equivalent=args.length>1 && args[1].equals("equivalent");
        boolean late=args.length>1 && args[1].startsWith("late");
        boolean lateMulti=args.length>1 && args[1].equals("late_multi");
        w.lateFar=args.length>1 && args[1].equals("late_far");
        w.mixed=args.length>1 && (args[1].equals("mixed") || equivalent || late);
        int radius=(w.mixed || w.wide)?2:1;
        FakeChunk eastSection=null, westSection=null, farSection=null, centerSection=null;
        for(int z=-radius;z<=radius;z++) for(int x=-radius;x<=radius;x++){
            var chunk=w.new TestChunk(x,z); w.chunks.put(new ChunkPos(x,z),chunk);
            var fake=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
            var container=new PalettedContainer<>(Blocks.AIR.defaultBlockState(),Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
            fake.section=new LevelChunkSection(container,null);
            if(x==1 && z==0) eastSection=fake;
            if(x==0 && z==0) centerSection=fake;
            if(x==-1 && z==0) westSection=fake;
            if(x==2 && z==0) farSection=fake;
            if(!((treeEmpty || treeSparse || vineActiveEmpty) && x==1 && z==0) && !(w.emptyWest && x==-1 && z==0)) for(int y=0;y<8;y++) for(int lz=0;lz<16;lz++) for(int lx=0;lx<16;lx++) fake.section.setBlockState(lx,y,lz,Blocks.STONE.defaultBlockState());
            if(treeSparse && x==1 && z==0) fake.section.setBlockState(w.sparseColumn || skyShadow?0:15,skyShadow?11:0,w.sparseColumn || skyShadow?8:0,Blocks.STONE.defaultBlockState());
            if(args.length>1 && args[1].equals("log_active_sparse_litter_wide") && x==1 && z==0) {
                var pos=new BlockPos(16,10,8); var litter=Blocks.LEAF_LITTER.defaultBlockState();
                fake.section.setBlockState(0,10,8,litter); w.edits.put(pos,litter);
            }
            if(skyMany && x==1 && z==0){ fake.section.setBlockState(0,11,9,Blocks.STONE.defaultBlockState()); fake.section.setBlockState(1,11,8,Blocks.STONE.defaultBlockState()); }
            if(x==1 && z==0 && !blockCase && !magma && !treeEmpty && !treeSparse && !vineActiveEmpty) fake.section.setBlockState(1,10,8,Blocks.TORCH.defaultBlockState());
            if(x==1 && z==0 && lichenActive) {
                var water=Blocks.WATER.defaultBlockState();
                fake.section.setBlockState(0,10,8,water); w.edits.put(new BlockPos(16,10,8),water);
            }
            if(x==1 && z==0 && patchFreshWater) {
                fake.section.setBlockState(0,10,8,Blocks.CLAY.defaultBlockState());
                w.edits.put(new BlockPos(16,10,8),Blocks.CLAY.defaultBlockState());
            }
            if(x==1 && z==0 && (freshWaterDripleaf || freshDripleafSequence)) {
                fake.section.setBlockState(0,10,8,Blocks.WATER.defaultBlockState());
                w.edits.put(new BlockPos(16,10,8),Blocks.WATER.defaultBlockState());
            }
            if(x==1 && z==0 && freshWaterlogDripleaf) {
                var initial=Blocks.BIG_DRIPLEAF.defaultBlockState().setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.HORIZONTAL_FACING,net.minecraft.core.Direction.NORTH).setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.WATERLOGGED,false);
                fake.section.setBlockState(0,10,8,initial);
                w.edits.put(new BlockPos(16,10,8),initial);
            }
            if(blockCase && x==0 && z==0){
                fake.section.setBlockState(12,10,8,Blocks.TORCH.defaultBlockState()); fake.section.setBlockState(14,10,8,Blocks.TORCH.defaultBlockState());
                if(w.stairSky){
                    for(int lx=11;lx<=15;lx++) for(int y=9;y<=11;y++) for(int lz=7;lz<=9;lz++) {
                        var p=new BlockPos(lx,y,lz);
                        var block=((y==10 && lz==8) || (lx==12 && y==11 && lz==8))?Blocks.AIR.defaultBlockState():Blocks.STONE.defaultBlockState();
                        fake.section.setBlockState(lx,y,lz,block);w.edits.put(p,block);
                    }
                    var stair=Blocks.OAK_STAIRS.defaultBlockState();
                    fake.section.setBlockState(13,10,8,stair);w.edits.put(new BlockPos(13,10,8),stair);
                    var neighbor=stair.setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.HORIZONTAL_FACING,Direction.SOUTH);
                    fake.section.setBlockState(14,10,8,neighbor);w.edits.put(new BlockPos(14,10,8),neighbor);
                } else if(w.stairCase){
                    for(int lx=11;lx<=15;lx++) for(int y=9;y<=11;y++) for(int lz=7;lz<=9;lz++) {
                        var p=new BlockPos(lx,y,lz);
                        var block=(y==10 && lz==8)?Blocks.AIR.defaultBlockState():Blocks.STONE.defaultBlockState();
                        fake.section.setBlockState(lx,y,lz,block);w.edits.put(p,block);
                    }
                    fake.section.setBlockState(12,10,8,Blocks.TORCH.defaultBlockState());w.edits.put(new BlockPos(12,10,8),Blocks.TORCH.defaultBlockState());
                    var stair=Blocks.OAK_STAIRS.defaultBlockState();
                    fake.section.setBlockState(13,10,8,stair);
                    w.edits.put(new BlockPos(13,10,8),stair);
                    var neighbor=stair.setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.HORIZONTAL_FACING,Direction.SOUTH);
                    fake.section.setBlockState(14,10,8,neighbor);w.edits.put(new BlockPos(14,10,8),neighbor);
                }
            }
            if(x==1 && z==0 && w.lateFar) fake.section.setBlockState(15,10,8,Blocks.TORCH.defaultBlockState());
            if(x==1 && z==0 && existing){
                var initial=Blocks.CAVE_VINES_PLANT.defaultBlockState().setValue(net.minecraft.world.level.block.CaveVines.BERRIES,false);
                fake.section.setBlockState(0,10,8,initial);
                w.edits.put(new BlockPos(16,10,8),initial);
            }
            if(x==1 && z==0 && magma) fake.section.setBlockState(0,8,8,Blocks.WATER.defaultBlockState());
            if(x==1 && z==0 && exposed) { fake.section.setBlockState(1,7,8,Blocks.AIR.defaultBlockState()); w.edits.put(new BlockPos(17,7,8),Blocks.AIR.defaultBlockState()); }
            if(w.mixed && x==0 && z==0) fake.section.setBlockState(0,10,8,Blocks.TORCH.defaultBlockState());
            if((!partial && !w.mixed) || (x==1 && z==0 && !late) || ((w.mixed || emptyPartialWide) && x==0 && z==0)){
                chunk.sources.fillFrom(fake);
                engine.updateSectionStatus(SectionPos.of(x,0,z),fake.section.hasOnlyAir());
            }
        }
        drain(engine); var stages=new JsonArray();
        if(args.length>1 && (args[1].startsWith("tree_fresh_") || patchFreshWater || freshWaterDripleaf || freshWaterlogDripleaf || freshDripleafSequence)){
            var before=new JsonArray(); before.add(capture(engine,0)); before.add(capture(engine,1)); stages.add(before);
            var state=(freshWaterDripleaf || freshWaterlogDripleaf || freshDripleafSequence) ? Blocks.BIG_DRIPLEAF.defaultBlockState().setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.HORIZONTAL_FACING,net.minecraft.core.Direction.NORTH).setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.WATERLOGGED,freshWaterlogDripleaf) : patchFreshWater ? Blocks.WATER.defaultBlockState() : args[1].equals("tree_fresh_birch_log") ? Blocks.BIRCH_LOG.defaultBlockState() : args[1].equals("tree_fresh_birch_leaf") ? Blocks.BIRCH_LEAVES.defaultBlockState() : (args[1].equals("tree_fresh_log") || treeFreshEmpty) ? Blocks.DARK_OAK_LOG.defaultBlockState() : Blocks.DARK_OAK_LEAVES.defaultBlockState();
            var pos=new BlockPos(16,10,8);
            boolean wasEmpty=eastSection.section.hasOnlyAir();
            var old=eastSection.section.setBlockState(0,10,8,state); w.edits.put(pos,state);
            if(wasEmpty!=eastSection.section.hasOnlyAir()) engine.updateSectionStatus(SectionPos.of(1,0,0),eastSection.section.hasOnlyAir());
            w.chunks.get(new ChunkPos(1,0)).sources.update(eastSection,0,10,8);
            if(!LightEngine.hasDifferentLightProperties(old,state))throw new AssertionError("missing tree light check");
            engine.checkBlock(pos);
            if(freshDripleafSequence) {
                var wet=state.setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.WATERLOGGED,true);
                var intermediate=eastSection.section.setBlockState(0,10,8,wet); w.edits.put(pos,wet);
                w.chunks.get(new ChunkPos(1,0)).sources.update(eastSection,0,10,8);
                if(!LightEngine.hasDifferentLightProperties(intermediate,wet))throw new AssertionError("missing waterlog light check");
                engine.checkBlock(pos);
            }
            drain(engine);
            var checked=new JsonArray(); checked.add(capture(engine,0)); checked.add(capture(engine,1)); stages.add(checked);
            engine.propagateLightSources(new ChunkPos(0,0)); drain(engine);
            var active=new JsonArray(); active.add(capture(engine,0)); active.add(capture(engine,1)); stages.add(active);
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        if(blockCase){
            engine.propagateLightSources(new ChunkPos(0,0)); drain(engine); stages.add(captureWide(engine));
            String mode=args[1];
            int[][] positions=mode.equals("block_incremental_activate_many") ? new int[][]{{16,10,8},{17,10,8},{30,10,4}} :
                mode.equals("block_incremental_many") ? new int[][]{{14,10,8},{15,10,8},{16,10,8}} :
                mode.startsWith("block_incremental_stair_") ? new int[][]{{13,10,8}} :
                mode.equals("block_incremental_remove") ? new int[][]{{14,10,8}} :
                mode.equals("block_incremental_obstruct") ? new int[][]{{15,10,8}} :
                mode.equals("block_incremental_add_active") ? new int[][]{{13,10,9}} : new int[][]{{16,10,8}};
            for(var p:positions){
                var pos=new BlockPos(p[0],p[1],p[2]); var fake=p[0]<16 ? centerSection : eastSection;
                BlockState state=mode.startsWith("block_incremental_stair_") ? Blocks.OAK_STAIRS.defaultBlockState().setValue(
                    net.minecraft.world.level.block.state.properties.BlockStateProperties.STAIRS_SHAPE,
                    (mode.endsWith("outer") || mode.endsWith("sequence")) ? net.minecraft.world.level.block.state.properties.StairsShape.OUTER_LEFT : net.minecraft.world.level.block.state.properties.StairsShape.INNER_RIGHT) :
                    (p[0]==14 && p[2]==8) ? Blocks.AIR.defaultBlockState() :
                    (p[0]==15 && p[2]==8) ? Blocks.STONE.defaultBlockState() : Blocks.TORCH.defaultBlockState();
                var old=fake.section.setBlockState(Math.floorMod(p[0],16),p[1],p[2],state); w.edits.put(pos,state);
                if(!LightEngine.hasDifferentLightProperties(old,state))throw new AssertionError("missing block light check");
                w.chunks.get(new ChunkPos(Math.floorDiv(p[0],16),0)).sources.update(fake,Math.floorMod(p[0],16),p[1],p[2]);
                engine.checkBlock(pos);
            }
            if(mode.equals("block_incremental_many"))engine.checkBlock(new BlockPos(14,10,8));
            stages.add(captureWide(engine)); drain(engine); stages.add(captureWide(engine));
            if(mode.equals("block_incremental_stair_sequence")){
                var pos=new BlockPos(13,10,8);
                var state=Blocks.OAK_STAIRS.defaultBlockState().setValue(
                    net.minecraft.world.level.block.state.properties.BlockStateProperties.STAIRS_SHAPE,
                    net.minecraft.world.level.block.state.properties.StairsShape.INNER_RIGHT);
                var old=centerSection.section.setBlockState(13,10,8,state);w.edits.put(pos,state);
                if(!LightEngine.hasDifferentLightProperties(old,state))throw new AssertionError("missing second stair light check");
                w.chunks.get(new ChunkPos(0,0)).sources.update(centerSection,13,10,8);
                engine.checkBlock(pos);stages.add(captureWide(engine));drain(engine);stages.add(captureWide(engine));
            }
            if(mode.startsWith("block_incremental_activate")){
                engine.propagateLightSources(new ChunkPos(1,0)); stages.add(captureWide(engine));
                drain(engine); stages.add(captureWide(engine));
            }
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        if(leafActive){
            engine.propagateLightSources(new ChunkPos(activeEmptyWide?1:leafFull?0:1,0)); drain(engine); stages.add(skyWide ? captureWide(engine) : capture(engine));
            var state=vineNoSky ? Blocks.CAVE_VINES_PLANT.defaultBlockState().setValue(net.minecraft.world.level.block.CaveVines.BERRIES,true) : emptyLeaf ? Blocks.DARK_OAK_LEAVES.defaultBlockState() : (logActive || treeEmpty || treeSparse) ? Blocks.DARK_OAK_LOG.defaultBlockState() : Blocks.DARK_OAK_LEAVES.defaultBlockState();
            int[][] positions=skyMany ? new int[][]{{16,10,8},{16,10,9},{17,10,8}} : leafMany ? new int[][]{{16,10,8},{17,11,8},{31,10,4},{16,9,10}} : new int[][]{{16,10,8}};
            for(var p:positions){
                var pos=new BlockPos(p[0],p[1],p[2]); boolean wasEmpty=eastSection.section.hasOnlyAir();
                var old=eastSection.section.setBlockState(p[0]-16,p[1],p[2],state); w.edits.put(pos,state);
                if(wasEmpty!=eastSection.section.hasOnlyAir()) {
                    if(!treeEmpty)throw new AssertionError("unexpected section transition");
                    engine.updateSectionStatus(SectionPos.of(1,0,0),eastSection.section.hasOnlyAir());
                }
                w.chunks.get(new ChunkPos(1,0)).sources.update(eastSection,p[0]-16,p[1],p[2]);
                if(!LightEngine.hasDifferentLightProperties(old,state))throw new AssertionError("missing leaf light check");
                engine.checkBlock(pos);
            }
            if(skyMany)engine.checkBlock(new BlockPos(16,10,8));
            if(skyWide && args.length>2) Files.writeString(Path.of(args[2]),pendingSkyChecks(engine).toString());
            stages.add(skyWide ? captureWide(engine) : capture(engine)); drain(engine); stages.add(skyWide ? captureWide(engine) : capture(engine));
            if(stagedActivate){ engine.propagateLightSources(new ChunkPos(1,0)); stages.add(captureWide(engine)); drain(engine); stages.add(captureWide(engine)); }
            if(skyActivate){ engine.propagateLightSources(new ChunkPos(1,0)); stages.add(captureWide(engine)); drain(engine); stages.add(captureWide(engine)); }
            if(registerAfter){
                w.chunks.get(new ChunkPos(-1,0)).sources.fillFrom(westSection);
                engine.updateSectionStatus(SectionPos.of(-1,0,0),westSection.section.hasOnlyAir());
                stages.add(captureWide(engine)); drain(engine); stages.add(captureWide(engine));
            }
            if(args.length>1 && args[1].equals("log_active_sparse_twice")){
                var pos=new BlockPos(16,10,9);
                var old=eastSection.section.setBlockState(0,10,9,Blocks.DARK_OAK_LOG.defaultBlockState());
                w.edits.put(pos,Blocks.DARK_OAK_LOG.defaultBlockState());
                w.chunks.get(new ChunkPos(1,0)).sources.update(eastSection,0,10,9);
                if(!LightEngine.hasDifferentLightProperties(old,Blocks.DARK_OAK_LOG.defaultBlockState()))throw new AssertionError("missing second light check");
                engine.checkBlock(pos); stages.add(capture(engine)); drain(engine); stages.add(capture(engine));
            }
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        if(vineActive){
            engine.propagateLightSources(new ChunkPos(vineSelf?1:0,0)); drain(engine); stages.add(captureWide(engine));
            int[][] positions=vineActiveTransition ? new int[][]{{16,10,8},{16,10,8}} : vineActiveMany ? new int[][]{{16,10,8},{16,9,8},{16,8,8}} : new int[][]{{16,10,8}};
            for(int i=0;i<positions.length;i++){
                var p=positions[i]; var pos=new BlockPos(p[0],p[1],p[2]);
                boolean berries=vineActiveTransition ? i==1 : vineActiveMany ? i!=1 : !plain;
                var block=args[1].equals("vine_active_head") ? Blocks.CAVE_VINES : Blocks.CAVE_VINES_PLANT;
                var state=lichenActive
                    ? Blocks.GLOW_LICHEN.defaultBlockState().setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.WATERLOGGED,true).setValue(net.minecraft.world.level.block.MultifaceBlock.getFaceProperty(net.minecraft.core.Direction.UP),true)
                    : block.defaultBlockState().setValue(net.minecraft.world.level.block.CaveVines.BERRIES,berries);
                if(args[1].equals("vine_active_head")) state=state.setValue(net.minecraft.world.level.block.GrowingPlantHeadBlock.AGE,7);
                boolean wasEmpty=eastSection.section.hasOnlyAir();
                var old=eastSection.section.setBlockState(0,p[1],p[2],state); w.edits.put(pos,state);
                if(wasEmpty!=eastSection.section.hasOnlyAir()) engine.updateSectionStatus(SectionPos.of(1,0,0),eastSection.section.hasOnlyAir());
                w.chunks.get(new ChunkPos(1,0)).sources.update(eastSection,0,p[1],p[2]);
                boolean changed=LightEngine.hasDifferentLightProperties(old,state);
                if(changed!=(lichenActive || berries))throw new AssertionError("unexpected active vine light check");
                if(changed)engine.checkBlock(pos);
            }
            stages.add(captureWide(engine)); drain(engine); stages.add(captureWide(engine));
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        if(vine){
            var pos=new BlockPos(16,10,8);
            var state=Blocks.CAVE_VINES_PLANT.defaultBlockState().setValue(net.minecraft.world.level.block.CaveVines.BERRIES,!plain);
            boolean wasEmpty=eastSection.section.hasOnlyAir();
            var old=eastSection.section.setBlockState(0,10,8,state);
            w.edits.put(pos,state);
            boolean isEmpty=eastSection.section.hasOnlyAir();
            if(wasEmpty!=isEmpty)throw new AssertionError("section status changed");
            w.chunks.get(new ChunkPos(1,0)).sources.update(eastSection,0,10,8);
            boolean changed=LightEngine.hasDifferentLightProperties(old,state);
            if(changed==plain)throw new AssertionError("unexpected vine light check");
            if(changed)engine.checkBlock(pos);
            stages.add(captureNearby(engine,full));if(!queued)drain(engine);stages.add(captureNearby(engine,full));
            engine.propagateLightSources(new ChunkPos(1,0));drain(engine);stages.add(captureNearby(engine,full));
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        if(magma){
            var pos=new BlockPos(16,7,8);var old=eastSection.section.setBlockState(0,7,8,Blocks.MAGMA_BLOCK.defaultBlockState());
            w.edits.put(pos,Blocks.MAGMA_BLOCK.defaultBlockState());
            if(!LightEngine.hasDifferentLightProperties(old,Blocks.MAGMA_BLOCK.defaultBlockState()))throw new AssertionError("missing light check");
            w.chunks.get(new ChunkPos(1,0)).sources.update(eastSection,0,7,8);
            engine.checkBlock(pos);stages.add(capture(engine));if(!queued)drain(engine);stages.add(capture(engine));
            engine.propagateLightSources(new ChunkPos(1,0));drain(engine);stages.add(capture(engine));
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        if(normal){
            // Mirror ProtoChunk.setBlockState's section and light checks.
            boolean wasEmpty=eastSection.section.hasOnlyAir();
            var old=eastSection.section.setBlockState(1,4,8,Blocks.SAND.defaultBlockState());
            w.edits.put(new BlockPos(17,4,8),Blocks.SAND.defaultBlockState());
            boolean isEmpty=eastSection.section.hasOnlyAir();
            if(wasEmpty!=isEmpty) engine.updateSectionStatus(SectionPos.of(1,0,0),isEmpty);
            if(LightEngine.hasDifferentLightProperties(old,Blocks.SAND.defaultBlockState())) throw new AssertionError("unexpected light check");
            drain(engine); stages.add(capture(engine));
            engine.propagateLightSources(new ChunkPos(1,0)); drain(engine); stages.add(capture(engine));
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        if(ore){
            if(oreActive){
                if(LightEngine.hasDifferentLightProperties(Blocks.DEEPSLATE.defaultBlockState(),Blocks.TUFF.defaultBlockState()))throw new AssertionError("deepslate and tuff light properties differ");
                for(var litter:Blocks.LEAF_LITTER.getStateDefinition().getPossibleStates())
                    if(LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(),litter))throw new AssertionError("leaf litter requires light check: "+litter);
                engine.propagateLightSources(new ChunkPos(1,0)); drain(engine); stages.add(capture(engine));
            }
            // OreFeature writes the section directly after INITIALIZE_LIGHT.
            eastSection.section.setBlockState(1,4,8,Blocks.DIAMOND_ORE.defaultBlockState(),false);
            w.edits.put(new BlockPos(17,4,8),Blocks.DIAMOND_ORE.defaultBlockState());
            drain(engine); stages.add(capture(engine));
            if(!oreActive){ engine.propagateLightSources(new ChunkPos(1,0)); drain(engine); stages.add(capture(engine)); }
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        if(w.mixed){
            engine.propagateLightSources(new ChunkPos(0,0)); drain(engine);
            stages.add(captureMixed(engine,false));
            if(late){
                if(w.lateFar){
                    w.edits.put(new BlockPos(32,10,8),Blocks.STONE.defaultBlockState());
                    w.edits.put(new BlockPos(33,10,8),Blocks.TORCH.defaultBlockState());
                    farSection.section.setBlockState(0,10,8,Blocks.STONE.defaultBlockState());
                    farSection.section.setBlockState(1,10,8,Blocks.TORCH.defaultBlockState());
                }
                w.chunks.get(new ChunkPos(1,0)).sources.fillFrom(eastSection);
                engine.updateSectionStatus(SectionPos.of(1,0,0),false); drain(engine);
                stages.add(captureMixed(engine,w.lateFar));
                engine.propagateLightSources(new ChunkPos(1,0)); drain(engine);
                stages.add(captureMixed(engine,w.lateFar));
                if(w.lateFar){
                    w.chunks.get(new ChunkPos(2,0)).sources.fillFrom(farSection);
                    engine.updateSectionStatus(SectionPos.of(2,0,0),false); drain(engine);
                    stages.add(captureMixed(engine,true));
                    engine.propagateLightSources(new ChunkPos(2,0)); drain(engine);
                    stages.add(captureMixed(engine,true));
                }
                if(lateMulti){
                    w.chunks.get(new ChunkPos(-1,0)).sources.fillFrom(westSection);
                    engine.updateSectionStatus(SectionPos.of(-1,0,0),false); drain(engine);
                    var registeredWest=new JsonArray(); registeredWest.add(capture(engine,0)); registeredWest.add(capture(engine,-1)); registeredWest.add(capture(engine,0,-1)); stages.add(registeredWest);
                    engine.propagateLightSources(new ChunkPos(-1,0)); drain(engine);
                    var activatedWest=new JsonArray(); activatedWest.add(capture(engine,0)); activatedWest.add(capture(engine,-1)); activatedWest.add(capture(engine,0,-1)); stages.add(activatedWest);
                }
                Files.writeString(Path.of(args[0]),stages.toString()); return;
            }
            if(equivalent) w.edits.put(new BlockPos(15,10,-1),Blocks.RED_MUSHROOM.defaultBlockState());
            else w.edits.put(new BlockPos(-1,10,8),Blocks.STONE.defaultBlockState());
            engine.propagateLightSources(new ChunkPos(1,0)); drain(engine);
            var second=new JsonArray(); second.add(capture(engine,0)); second.add(capture(engine,-1)); second.add(capture(engine,0,-1)); stages.add(second);
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        if(partial){
            engine.propagateLightSources(new ChunkPos(1,0)); drain(engine); stages.add(capture(engine));
            // Before INITIALIZE_LIGHT, ProtoChunk.setBlockState neither calls
            // updateSectionStatus nor checkBlock on the light engine.
            w.edits.put(new BlockPos(15,10,8),Blocks.STONE.defaultBlockState());
            drain(engine); stages.add(capture(engine));
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        stages.add(capture(engine));
        for(int x:new int[]{0,1}){ engine.propagateLightSources(new ChunkPos(x,0)); drain(engine); stages.add(capture(engine)); }
        Files.writeString(Path.of(args[0]),stages.toString());
    }
}
