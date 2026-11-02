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
    public int getMinY(){ return 0; }
    public int getHeight(){ return 16; }
    public BlockGetter getLevel(){ return this; }
    public LightChunk getChunkForLighting(int x,int z){ return chunks.get(new ChunkPos(x,z)); }
    public BlockEntity getBlockEntity(BlockPos p){ return null; }
    public BlockState getBlockState(BlockPos p){
        if(p.getX()==17 && p.getY()==10 && p.getZ()==8) return Blocks.TORCH.defaultBlockState();
        if(p.getY()>=0 && p.getY()<8 && Math.abs(Math.floorDiv(p.getX(),16))<=1 && Math.abs(Math.floorDiv(p.getZ(),16))<=1) return Blocks.STONE.defaultBlockState();
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
        public void findBlockLightSources(BiConsumer<BlockPos,BlockState> f){ if(x==1 && z==0){ var p=new BlockPos(17,10,8); f.accept(p,Blocks.TORCH.defaultBlockState()); } }
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
    static JsonArray capture(LevelLightEngine e){
        var result=new JsonArray();
        for(var layer:new LightLayer[]{LightLayer.BLOCK,LightLayer.SKY}){
            var values=new JsonArray(); var listener=e.getLayerListener(layer);
            for(int y=-16;y<32;y++) for(int z=0;z<16;z++) for(int x=0;x<16;x++) values.add(listener.getLightValue(new BlockPos(x,y,z)));
            result.add(values);
        }
        return result;
    }
    public static void main(String[] args)throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var w=new StagedGenerationLightEngineOracle();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var engine=new LevelLightEngine(w,true,true);
        for(int z=-1;z<=1;z++) for(int x=-1;x<=1;x++){
            var chunk=w.new TestChunk(x,z); w.chunks.put(new ChunkPos(x,z),chunk);
            var fake=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
            var container=new PalettedContainer<>(Blocks.AIR.defaultBlockState(),Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
            fake.section=new LevelChunkSection(container,null);
            for(int y=0;y<8;y++) for(int lz=0;lz<16;lz++) for(int lx=0;lx<16;lx++) fake.section.setBlockState(lx,y,lz,Blocks.STONE.defaultBlockState());
            if(x==1 && z==0) fake.section.setBlockState(1,10,8,Blocks.TORCH.defaultBlockState());
            chunk.sources.fillFrom(fake);
            engine.updateSectionStatus(SectionPos.of(x,0,z),false);
        }
        drain(engine); var stages=new JsonArray(); stages.add(capture(engine));
        for(int x:new int[]{0,1}){ engine.propagateLightSources(new ChunkPos(x,0)); drain(engine); stages.add(capture(engine)); }
        Files.writeString(Path.of(args[0]),stages.toString());
    }
}
