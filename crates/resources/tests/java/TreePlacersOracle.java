import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.util.valueproviders.ConstantInt;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.TreeFeature;
import net.minecraft.world.level.levelgen.feature.foliageplacers.BlobFoliagePlacer;
import net.minecraft.world.level.levelgen.feature.foliageplacers.FancyFoliagePlacer;
import net.minecraft.world.level.levelgen.feature.foliageplacers.FoliagePlacer;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.feature.trunkplacers.StraightTrunkPlacer;

public class TreePlacersOracle {
  static boolean obstacles;
  static boolean blocked(BlockPos p){return obstacles && Math.floorMod(p.getX()*3+p.getY()*5+p.getZ()*7,11)==0;}
  @SuppressWarnings("unchecked")
  public static void main(String[] args) {
    var out=System.out;
    SharedConstants.tryDetectVersion();
    Bootstrap.bootStrap();
    var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,method,values)->{
      if(method.getName().equals("isStateAtPosition"))
        return ((Predicate<BlockState>)values[1]).test((blocked((BlockPos)values[0])?Blocks.STONE:Blocks.AIR).defaultBlockState());
      if(method.getName().equals("isFluidAtPosition"))return false;
      throw new UnsupportedOperationException(method.toString());
    });
    var leafProvider=(BlockStateProvider)Proxy.newProxyInstance(BlockStateProvider.class.getClassLoader(),new Class[]{BlockStateProvider.class},(proxy,method,values)->{
      if(method.getName().equals("getState")){((net.minecraft.util.RandomSource)values[1]).nextInt(17);return Blocks.OAK_LEAVES.defaultBlockState();}
      throw new UnsupportedOperationException(method.toString());
    });
    var tree=new TreeFeature(BlockStateProvider.holderOf(Blocks.OAK_LOG),null,
      Holder.direct(leafProvider),null,Optional.empty(),null,List.of(),true,
      BlockStateProvider.holderOf(Blocks.DIRT));
    for(long seed:new long[]{0,1,-1,987654321})
      for(int height:new int[]{0,1,3,5})
        for(int radius:new int[]{0,2,4})
          for(int offset:new int[]{0,1})
            for(boolean big:new boolean[]{false,true})
              for(boolean obstructed:new boolean[]{false,true}) {
              obstacles=obstructed;
              var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
              var trunk=new StraightTrunkPlacer(4,2,0);
              int trunkHeight=trunk.getTreeHeight(random);
              var trunks=new ArrayList<String>();
              var attachments=trunk.placeTrunk(world,(p,s)->trunks.add(p.getX()+","+p.getY()+","+p.getZ()),random,trunkHeight,new BlockPos(3,10,-4),tree);
              if(attachments.size()!=1 || !attachments.get(0).pos().equals(new BlockPos(3,10+trunkHeight,-4)))throw new AssertionError("trunk attachment");
              var blob=new BlobFoliagePlacer(ConstantInt.of(radius),ConstantInt.of(offset),height);
              var positions=new ArrayList<String>();
              var setter=new FoliagePlacer.FoliageSetter(){
                public void set(BlockPos p,BlockState s){positions.add(p.getX()+","+p.getY()+","+p.getZ());}
                public boolean isSet(BlockPos p){return false;}
              };
              int size=big?2:1;
              var attachment=new FoliagePlacer.FoliageAttachment(new BlockPos(3,30,-4),1,1,size,size);
              blob.createFoliage(world,setter,random,tree,trunkHeight,attachment,height,radius);
              out.printf("C %d %d %d %d %s %s %d %s L %s R %d%n",seed,height,radius,offset,big,obstructed,trunkHeight,String.join(";",trunks),String.join(";",positions),random.nextLong());
            }
    for(long seed:new long[]{0,1,-1,987654321})
      for(int height:new int[]{0,1,3,5})for(int radius:new int[]{0,2,4})for(int offset:new int[]{0,1,4})
        for(boolean big:new boolean[]{false,true})for(boolean obstructed:new boolean[]{false,true}){
          obstacles=obstructed;var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
          var fancy=new FancyFoliagePlacer(ConstantInt.of(radius),ConstantInt.of(offset),height);
          var positions=new ArrayList<String>();
          var setter=new FoliagePlacer.FoliageSetter(){
            public void set(BlockPos p,BlockState s){positions.add(p.getX()+","+p.getY()+","+p.getZ());}
            public boolean isSet(BlockPos p){return false;}
          };
          int size=big?2:1;
          var attachment=new FoliagePlacer.FoliageAttachment(new BlockPos(3,30,-4),1,1,size,size);
          fancy.createFoliage(world,setter,random,tree,7,attachment,height,radius);
          out.printf("F %d %d %d %d %s %s L %s R %d%n",seed,height,radius,offset,big,obstructed,String.join(";",positions),random.nextLong());
        }
  }
}
