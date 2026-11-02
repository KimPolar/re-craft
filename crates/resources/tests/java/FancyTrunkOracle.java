import java.lang.reflect.*;
import java.util.*;
import java.util.function.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.TreeFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.trunkplacers.FancyTrunkPlacer;

public class FancyTrunkOracle {
 static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
 @SuppressWarnings("unchecked") public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
  bind.invoke(Blocks.OAK_LOG.builtInRegistryHolder(),List.of(BlockTags.LOGS));
  for(long seed:new long[]{0,1,-1,987654321})for(int height:new int[]{0,1,3,4,6,10,20,40,64,80})for(int mode=0;mode<4;mode++){
   final int scenario=mode;var blocks=new HashMap<BlockPos,BlockState>();var writes=new ArrayList<String>();var queries=new ArrayList<String>();
   var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(p,m,v)->{
    if(m.getName().equals("isStateAtPosition")){var q=(BlockPos)v[0];queries.add(pos(q));var s=blocks.get(q);if(s==null)s=(((scenario&1)!=0 && Math.floorMod(q.getX()*3+q.getY()*5+q.getZ()*7,17)==0)?Blocks.STONE:Blocks.AIR).defaultBlockState();return ((Predicate<BlockState>)v[1]).test(s);}
    throw new UnsupportedOperationException(m.toString());
   });
   var provider=(BlockStateProvider)Proxy.newProxyInstance(BlockStateProvider.class.getClassLoader(),new Class[]{BlockStateProvider.class},(p,m,v)->{
    if(m.getName().equals("getState")){if((scenario&2)!=0)((net.minecraft.util.RandomSource)v[1]).nextInt(17);return Blocks.OAK_LOG.defaultBlockState();}throw new UnsupportedOperationException(m.toString());
   });
   var tree=new TreeFeature(Holder.direct(provider),null,BlockStateProvider.holderOf(Blocks.OAK_LEAVES),null,Optional.empty(),null,List.of(),true,BlockStateProvider.holderOf(Blocks.DIRT));
   var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
   var placer=new FancyTrunkPlacer(3,11,0);
   var attachments=placer.placeTrunk(world,(p,s)->{p=p.immutable();blocks.put(p,s);writes.add(pos(p)+","+(s.is(Blocks.OAK_LOG)?s.getValue(BlockStateProperties.AXIS):"soil"));},random,height,new BlockPos(3,10,-4),tree);
   var points=attachments.stream().map(a->pos(a.pos())+","+a.radiusOffsetXZ()+","+a.foliageHeightOffset()+","+a.doubleTrunk()).toList();
   out.printf("C %d %d %d W %s A %s Q %s R %d%n",seed,height,mode,String.join(";",writes),String.join(";",points),String.join(";",queries),random.nextLong());
  }
 }
}
