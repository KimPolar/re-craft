import java.lang.reflect.*;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.LakeFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.ticks.*;

public class LakeFeatureOracle {
  static BlockState initial(int profile,BlockPos p){int y=p.getY();if(profile==0)return Blocks.STONE.defaultBlockState();if(profile==1&&y>=12)return Blocks.AIR.defaultBlockState();if(profile==2&&y>=12)return Blocks.WATER.defaultBlockState();if(profile==3&&Math.floorMod(p.getX()*7+y*3+p.getZ()*11,13)==0)return Blocks.BEDROCK.defaultBlockState();return Blocks.STONE.defaultBlockState();}
  static long key(int x,int z){return ((long)x<<32)^(z&0xffffffffL);}
  public static void main(String[]args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var biome=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));var ids=new IdMapper<Holder<Biome>>();ids.add(biome);
    var palette=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),biome,null);
    var height=LevelHeightAccessor.create(0,32);
    for(var block:List.of(Blocks.CAVE_AIR,Blocks.LAVA,Blocks.STONE,Blocks.ICE)){var s=block.defaultBlockState();out.printf("E %s %s %s %s%n",BuiltInRegistries.BLOCK.getKey(block),s.hasBlockEntity(),net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(s),s.getPostProcessPos(null,BlockPos.ZERO));}
    int number=0;
    for(int profile=0;profile<4;profile++)for(int restricted=0;restricted<2;restricted++)for(int mode=0;mode<3;mode++)for(long seed:new long[]{0,1,7,-1}) {
      var changes=new HashMap<BlockPos,BlockState>();var writes=new ArrayList<String>();var chunks=new TreeMap<Long,ProtoChunk>();var ticks=new TreeMap<Long,ProtoChunkTicks<Block>>();
      for(int z=-1;z<=0;z++)for(int x=-1;x<=0;x++){long key=key(x,z);var c=new ProtoChunk(new ChunkPos(x,z),UpgradeData.EMPTY,height,palette,null);c.setPersistedStatus(ChunkStatus.TERRAIN);chunks.put(key,c);ticks.put(key,new ProtoChunkTicks<>());}
      int current=profile,limit=restricted;
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
        if(m.getName().equals("getMinY"))return 0;
        var p=(BlockPos)a[0];var state=changes.getOrDefault(p,initial(current,p));long k=key(p.getX()>>4,p.getZ()>>4);
        return switch(m.getName()){
          case "getBlockState"->state;
          case "isStateAtPosition"->((Predicate<BlockState>)a[1]).test(state);
          case "getChunk"->{var c=chunks.get(k);if(c==null)throw new AssertionError("chunk "+p);yield c;}
          case "setBlock"->{if(limit==0){var s=(BlockState)a[1];changes.put(p.immutable(),s);writes.add(p.getX()+" "+p.getY()+" "+p.getZ()+" "+BuiltInRegistries.BLOCK.getKey(s.getBlock()));}yield limit==0;}
          case "scheduleTick"->{var t=ticks.get(k);if(t==null)throw new AssertionError("tick "+p);t.schedule(new ScheduledTick<>((Block)a[1],p,0L,0L));yield null;}
          default->throw new UnsupportedOperationException(m.getName());
        };
      });
      var canPlace=mode==1?BlockPredicate.not(BlockPredicate.matchesBlocks(Blocks.BEDROCK)):BlockPredicate.alwaysTrue();
      var replace=BlockPredicate.not(BlockPredicate.matchesBlocks(Blocks.BEDROCK));
      var barrier=mode==2?BlockStateProvider.of(Blocks.AIR):BlockStateProvider.of(Blocks.STONE);
      var feature=new LakeFeature(Holder.direct(BlockStateProvider.of(Blocks.LAVA)),Holder.direct(barrier),canPlace,replace,replace);
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));boolean result=feature.place(world,null,random,new BlockPos(0,12,0));
      out.printf("C %d %d %d %d %d %s %d%n",number++,profile,restricted,mode,seed,result,random.nextLong());
      for(var line:writes)out.println("B "+line);
      for(var e:chunks.entrySet()){var c=e.getValue();var p=c.getPos();var sections=c.getPostProcessing();for(int i=0;i<sections.length;i++)if(sections[i]!=null)for(short v:sections[i])out.printf("M %d %d %d %d%n",p.x(),p.z(),i,(int)v);}
      for(var e:ticks.entrySet())for(var t:e.getValue().scheduledTicks())out.printf("T %s %d %d %d %d %d%n",BuiltInRegistries.BLOCK.getKey(t.type()),t.pos().getX(),t.pos().getY(),t.pos().getZ(),t.delay(),t.priority().getValue());
    }
  }
}
