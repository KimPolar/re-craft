import java.util.*;
import java.util.function.BiConsumer;
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

public class VisibleGenerationLightOracle implements LightChunkGetter, LightChunk {
 final ChunkSkyLightSources sources=new ChunkSkyLightSources(this);
 final BlockPos torch=new BlockPos(8,8,8);
 public int getMinY(){return 0;}
 public int getHeight(){return 32;}
 public LightChunk getChunkForLighting(int x,int z){return this;}
 public BlockGetter getLevel(){return this;}
 public BlockEntity getBlockEntity(BlockPos p){return null;}
 public BlockState getBlockState(BlockPos p){return p.equals(torch)?Blocks.TORCH.defaultBlockState():Blocks.AIR.defaultBlockState();}
 public FluidState getFluidState(BlockPos p){return getBlockState(p).getFluidState();}
 public ChunkSkyLightSources getSkyLightSources(){return sources;}
 public void findBlockLightSources(BiConsumer<BlockPos,BlockState> f){f.accept(torch,Blocks.TORCH.defaultBlockState());}
 static void drain(LevelLightEngine e){while(e.hasLightWork())e.runLightUpdates();}
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var storageField=LightEngine.class.getDeclaredField("storage");storageField.setAccessible(true);
  var dataMethod=LayerLightSectionStorage.class.getDeclaredMethod("getDataLayer",long.class,boolean.class);dataMethod.setAccessible(true);
  for(int mode=0;mode<6;mode++)for(boolean block:new boolean[]{false,true})for(boolean sky:new boolean[]{false,true}) {
   var w=new VisibleGenerationLightOracle();var e=new LevelLightEngine(w,block,sky);
   if(mode==3){
    for(var layer:LightLayer.values()){var data=new DataLayer();for(int z=0;z<16;z++)for(int x=0;x<16;x++)for(int y=0;y<16;y++)data.set(x,y,z,(x+y*3+z*7+(layer==LightLayer.SKY?4:0))&15);e.queueSectionData(layer,SectionPos.of(0,0,0),data);}
   }
   if(mode>0){e.updateSectionStatus(SectionPos.of(0,0,0),false);drain(e);}
   if(mode==4){e.updateSectionStatus(SectionPos.of(-1,-1,1),false);e.updateSectionStatus(SectionPos.of(1,1,0),false);drain(e);}
   if(mode==5){e.updateSectionStatus(SectionPos.of(0,2,0),false);e.updateSectionStatus(SectionPos.of(1,-2,-1),false);drain(e);}
   if(mode==2){e.setLightEnabled(new ChunkPos(0,0),true);e.propagateLightSources(new ChunkPos(0,0));drain(e);}
   out.printf("H %d %s %s%n",mode,block,sky);
   for(var layer:LightLayer.values())for(int cz=-1;cz<=1;cz++)for(int cx=-1;cx<=1;cx++)for(int sy=-4;sy<=4;sy++){
    var listener=e.getLayerListener(layer);if(!(listener instanceof LightEngine))continue;
    var data=(DataLayer)dataMethod.invoke(storageField.get(listener),SectionPos.of(cx,sy,cz).asLong(),false);
    if(data!=null)out.printf("L %s %d %d %d %s%n",layer==LightLayer.BLOCK?"B":"S",cx,cz,sy,HexFormat.of().formatHex(data.getData()));
   }
   for(int x:new int[]{-15,-1,0,7,15,16,31})for(int z:new int[]{-1,0,7,16})for(int y:new int[]{-64,-17,-1,0,7,15,16,31,48,128}){
    var p=new BlockPos(x,y,z);int b=e.getLayerListener(LightLayer.BLOCK).getLightValue(p),s=e.getLayerListener(LightLayer.SKY).getLightValue(p);
    out.printf("Q %d %d %d %d %d %d %d%n",x,y,z,b,s,e.getRawBrightness(p,0),e.getRawBrightness(p,5));
   }
  }
 }
}
