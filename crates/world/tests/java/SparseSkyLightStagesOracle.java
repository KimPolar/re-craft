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

public class SparseSkyLightStagesOracle implements LightChunkGetter,BlockGetter {
    final Map<BlockPos,BlockState> blocks=new HashMap<>();
    final Map<ChunkPos,TestChunk> chunks=new HashMap<>();
    public int getMinY(){return 0;} public int getHeight(){return 96;}
    public BlockGetter getLevel(){return this;}
    public LightChunk getChunkForLighting(int x,int z){return chunks.get(new ChunkPos(x,z));}
    public BlockEntity getBlockEntity(BlockPos p){return null;}
    public BlockState getBlockState(BlockPos p){return blocks.getOrDefault(p,Blocks.AIR.defaultBlockState());}
    public FluidState getFluidState(BlockPos p){return getBlockState(p).getFluidState();}
    class TestChunk implements LightChunk {
        final int x,z; final ChunkSkyLightSources sources=new ChunkSkyLightSources(SparseSkyLightStagesOracle.this);
        TestChunk(int x,int z){this.x=x;this.z=z;}
        public BlockState getBlockState(BlockPos p){return SparseSkyLightStagesOracle.this.getBlockState(new BlockPos(x*16+Math.floorMod(p.getX(),16),p.getY(),z*16+Math.floorMod(p.getZ(),16)));}
        public FluidState getFluidState(BlockPos p){return getBlockState(p).getFluidState();}
        public BlockEntity getBlockEntity(BlockPos p){return null;}
        public int getMinY(){return 0;} public int getHeight(){return 96;}
        public ChunkSkyLightSources getSkyLightSources(){return sources;}
        public void findBlockLightSources(BiConsumer<BlockPos,BlockState> c){}
    }
    static class FakeChunk extends ProtoChunk {
        LevelChunkSection[] sections;
        FakeChunk(){super(null,null,null,null,null);}
        @Override public int getMinY(){return 0;} @Override public int getHeight(){return 96;}
        @Override public LevelChunkSection getSection(int i){return sections[i];}
        @Override public int getHighestFilledSectionIndex(){for(int i=sections.length-1;i>=0;i--)if(!sections[i].hasOnlyAir())return i;return -1;}
    }
    static JsonArray capture(LevelLightEngine e){
        var out=new JsonArray();var listener=e.getLayerListener(LightLayer.SKY);
        for(int y=-16;y<112;y++)for(int z=0;z<16;z++)for(int x=0;x<16;x++)out.add(listener.getLightValue(new BlockPos(x,y,z)));
        return out;
    }
    public static void main(String[] args)throws Exception {
        SharedConstants.tryDetectVersion();Bootstrap.bootStrap();var w=new SparseSkyLightStagesOracle();
        for(var item:JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray()){
            var o=item.getAsJsonObject();var p=new BlockPos(o.get("x").getAsInt(),o.get("y").getAsInt(),o.get("z").getAsInt());
            w.blocks.put(p, o.get("state").getAsString().equals("water")?Blocks.WATER.defaultBlockState():Blocks.STONE.defaultBlockState());
        }
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);var unsafe=(sun.misc.Unsafe)field.get(null);
        var e=new LevelLightEngine(w,false,true);
        for(int z=-1;z<=2;z++)for(int x=-1;x<=2;x++){
            var chunk=w.new TestChunk(x,z);w.chunks.put(new ChunkPos(x,z),chunk);
            var fake=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);fake.sections=new LevelChunkSection[6];
            for(int sy=0;sy<6;sy++){
                var container=new PalettedContainer<>(Blocks.AIR.defaultBlockState(),Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
                fake.sections[sy]=new LevelChunkSection(container,null);
            }
            for(var entry:w.blocks.entrySet()){
                var p=entry.getKey();
                if(Math.floorDiv(p.getX(),16)==x && Math.floorDiv(p.getZ(),16)==z)
                    fake.sections[p.getY()/16].setBlockState(Math.floorMod(p.getX(),16),p.getY()%16,Math.floorMod(p.getZ(),16),entry.getValue());
            }
            boolean initialized=args.length<=3 || Arrays.asList(args[3].split(",")).contains(x+":"+z);
            boolean registered=args.length<=4 ? initialized : Arrays.asList(args[4].split(",")).contains(x+":"+z);
            if(registered){
                for(int sy=0;sy<6;sy++)if(!fake.sections[sy].hasOnlyAir())e.updateSectionStatus(SectionPos.of(x,sy,z),false);
            }
            if(initialized)chunk.sources.fillFrom(fake);
        }
        while(e.hasLightWork())e.runLightUpdates();var out=new JsonArray();out.add(capture(e));
        var order=(args.length>2?args[2]:"0,1,-1").split(",");
        for(var value:order){int x=Integer.parseInt(value);e.propagateLightSources(new ChunkPos(x,0));while(e.hasLightWork())e.runLightUpdates();out.add(capture(e));}
        Files.writeString(Path.of(args[1]),out.toString());
    }
}
