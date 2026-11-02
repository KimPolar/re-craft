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
    public int getMinY(){ return 0; }
    public int getHeight(){ return 16; }
    public BlockGetter getLevel(){ return this; }
    public LightChunk getChunkForLighting(int x,int z){ return chunks.get(new ChunkPos(x,z)); }
    public BlockEntity getBlockEntity(BlockPos p){ return null; }
    public BlockState getBlockState(BlockPos p){
        if(edits.containsKey(p)) return edits.get(p);
        if(mixed && p.getX()==0 && p.getY()==10 && p.getZ()==8) return Blocks.TORCH.defaultBlockState();
        if(magmaOnly && p.getX()==16 && p.getY()==8 && p.getZ()==8) return Blocks.WATER.defaultBlockState();
        if(!magmaOnly && p.getX()==17 && p.getY()==10 && p.getZ()==8) return Blocks.TORCH.defaultBlockState();
        if(p.getY()>=0 && p.getY()<8 && Math.abs(Math.floorDiv(p.getX(),16))<=(mixed?2:1) && Math.abs(Math.floorDiv(p.getZ(),16))<=(mixed?2:1)) return Blocks.STONE.defaultBlockState();
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
            if(x==1 && z==0 && !magmaOnly) f.accept(new BlockPos(17,10,8),Blocks.TORCH.defaultBlockState());
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
    public static void main(String[] args)throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var w=new StagedGenerationLightEngineOracle();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var engine=new LevelLightEngine(w,true,true);
        boolean ore=args.length>1 && args[1].equals("ore");
        boolean normal=args.length>1 && args[1].equals("normal");
        boolean queued=args.length>1 && (args[1].equals("magma_queued") || args[1].equals("magma_exposed_queued") || args[1].equals("vine_queued") || args[1].equals("vine_plain_queued") || args[1].equals("vine_existing_queued"));
        boolean magma=args.length>1 && (args[1].equals("magma") || args[1].equals("magma_queued") || args[1].equals("magma_exposed") || args[1].equals("magma_exposed_queued"));
        boolean exposed=args.length>1 && args[1].startsWith("magma_exposed");
        boolean vine=args.length>1 && (args[1].equals("vine") || args[1].equals("vine_queued") || args[1].equals("vine_plain") || args[1].equals("vine_plain_queued") || args[1].equals("vine_existing") || args[1].equals("vine_existing_queued"));
        boolean plain=args.length>1 && args[1].startsWith("vine_plain");
        boolean existing=args.length>1 && args[1].startsWith("vine_existing");
        w.magmaOnly=magma;
        boolean partial=args.length>1 && (args[1].equals("partial") || ore || normal || magma || vine);
        boolean equivalent=args.length>1 && args[1].equals("equivalent");
        w.mixed=args.length>1 && (args[1].equals("mixed") || equivalent);
        int radius=w.mixed?2:1;
        FakeChunk eastSection=null;
        for(int z=-radius;z<=radius;z++) for(int x=-radius;x<=radius;x++){
            var chunk=w.new TestChunk(x,z); w.chunks.put(new ChunkPos(x,z),chunk);
            var fake=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
            var container=new PalettedContainer<>(Blocks.AIR.defaultBlockState(),Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
            fake.section=new LevelChunkSection(container,null);
            if(x==1 && z==0) eastSection=fake;
            for(int y=0;y<8;y++) for(int lz=0;lz<16;lz++) for(int lx=0;lx<16;lx++) fake.section.setBlockState(lx,y,lz,Blocks.STONE.defaultBlockState());
            if(x==1 && z==0 && !magma) fake.section.setBlockState(1,10,8,Blocks.TORCH.defaultBlockState());
            if(x==1 && z==0 && existing){
                var initial=Blocks.CAVE_VINES_PLANT.defaultBlockState().setValue(net.minecraft.world.level.block.CaveVines.BERRIES,false);
                fake.section.setBlockState(0,10,8,initial);
                w.edits.put(new BlockPos(16,10,8),initial);
            }
            if(x==1 && z==0 && magma) fake.section.setBlockState(0,8,8,Blocks.WATER.defaultBlockState());
            if(x==1 && z==0 && exposed) { fake.section.setBlockState(1,7,8,Blocks.AIR.defaultBlockState()); w.edits.put(new BlockPos(17,7,8),Blocks.AIR.defaultBlockState()); }
            if(w.mixed && x==0 && z==0) fake.section.setBlockState(0,10,8,Blocks.TORCH.defaultBlockState());
            if((!partial && !w.mixed) || (x==1 && z==0) || (w.mixed && x==0 && z==0)){
                chunk.sources.fillFrom(fake);
                engine.updateSectionStatus(SectionPos.of(x,0,z),false);
            }
        }
        drain(engine); var stages=new JsonArray();
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
            stages.add(capture(engine));if(!queued)drain(engine);stages.add(capture(engine));
            engine.propagateLightSources(new ChunkPos(1,0));drain(engine);stages.add(capture(engine));
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
            // OreFeature writes the section directly after INITIALIZE_LIGHT.
            eastSection.section.setBlockState(1,4,8,Blocks.DIAMOND_ORE.defaultBlockState(),false);
            w.edits.put(new BlockPos(17,4,8),Blocks.DIAMOND_ORE.defaultBlockState());
            drain(engine); stages.add(capture(engine));
            engine.propagateLightSources(new ChunkPos(1,0)); drain(engine); stages.add(capture(engine));
            Files.writeString(Path.of(args[0]),stages.toString()); return;
        }
        if(w.mixed){
            engine.propagateLightSources(new ChunkPos(0,0)); drain(engine);
            var first=new JsonArray(); first.add(capture(engine,0)); first.add(capture(engine,-1)); first.add(capture(engine,0,-1)); stages.add(first);
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
