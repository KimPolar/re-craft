import java.nio.file.*;
import java.util.*;
import java.util.function.BiConsumer;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.lighting.*;
import net.minecraft.world.level.material.FluidState;

public class InitialBlockLightStagesOracle implements LightChunkGetter, BlockGetter {
    final Map<BlockPos,BlockState> blocks=new HashMap<>();
    final Map<ChunkPos,TestChunk> chunks=new HashMap<>();
    public int getMinY(){return 0;} public int getHeight(){return 32;}
    public BlockGetter getLevel(){return this;}
    public LightChunk getChunkForLighting(int x,int z){return chunks.get(new ChunkPos(x,z));}
    public BlockEntity getBlockEntity(BlockPos p){return null;}
    public BlockState getBlockState(BlockPos p){return blocks.getOrDefault(p,Blocks.AIR.defaultBlockState());}
    public FluidState getFluidState(BlockPos p){return getBlockState(p).getFluidState();}
    class TestChunk implements LightChunk {
        final int x,z; TestChunk(int x,int z){this.x=x;this.z=z;}
        public BlockState getBlockState(BlockPos p){return InitialBlockLightStagesOracle.this.getBlockState(p);}
        public FluidState getFluidState(BlockPos p){return getBlockState(p).getFluidState();}
        public BlockEntity getBlockEntity(BlockPos p){return null;}
        public int getMinY(){return 0;} public int getHeight(){return 32;}
        public ChunkSkyLightSources getSkyLightSources(){return null;}
        public void findBlockLightSources(BiConsumer<BlockPos,BlockState> consumer){
            blocks.forEach((p,s)->{if(Math.floorDiv(p.getX(),16)==x && Math.floorDiv(p.getZ(),16)==z && s.getLightEmission()>0)consumer.accept(p,s);});
        }
    }
    static void drain(LevelLightEngine engine){while(engine.hasLightWork())engine.runLightUpdates();}
    static JsonArray capture(LevelLightEngine engine){
        var out=new JsonArray();var listener=engine.getLayerListener(LightLayer.BLOCK);
        for(int y=-16;y<48;y++)for(int z=0;z<16;z++)for(int x=0;x<16;x++)out.add(listener.getLightValue(new BlockPos(x,y,z)));
        return out;
    }
    public static void main(String[] args)throws Exception {
        SharedConstants.tryDetectVersion();Bootstrap.bootStrap();var w=new InitialBlockLightStagesOracle();
        for(int z=-1;z<=1;z++)for(int x=-1;x<=1;x++)w.chunks.put(new ChunkPos(x,z),w.new TestChunk(x,z));
        for(int[] p:new int[][]{{3,8,8},{16,8,8},{-1,8,5},{14,30,14}})w.blocks.put(new BlockPos(p[0],p[1],p[2]),Blocks.TORCH.defaultBlockState());
        for(int[] p:new int[][]{{4,8,8},{0,8,6},{15,8,7}})w.blocks.put(new BlockPos(p[0],p[1],p[2]),Blocks.STONE.defaultBlockState());
        var engine=new LevelLightEngine(w,true,false);
        for(int z=-1;z<=1;z++)for(int x=-1;x<=1;x++)for(int sy=0;sy<2;sy++)engine.updateSectionStatus(SectionPos.of(x,sy,z),false);
        drain(engine);var results=new JsonArray();results.add(capture(engine));
        for(int x:new int[]{0,1,-1}){engine.propagateLightSources(new ChunkPos(x,0));drain(engine);results.add(capture(engine));}
        Files.writeString(Path.of(args[0]),results.toString());
    }
}
