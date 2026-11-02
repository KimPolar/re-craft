import java.lang.reflect.Proxy;
import java.util.*;
import java.util.function.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.util.random.WeightedList;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.LeafLitterBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.stateproviders.*;
import net.minecraft.world.level.levelgen.feature.treedecorators.*;

public class TreeGroundOracle {
  @SuppressWarnings("unchecked") public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    for(long seed:new long[]{0,1,-1,987654321}) for(int mode=0;mode<5;mode++) {
      var blocks=new HashMap<BlockPos,BlockState>(); var writes=new ArrayList<String>();
      var logs=new LinkedHashSet<BlockPos>(); var roots=new LinkedHashSet<BlockPos>();
      if(mode!=3) logs.add(new BlockPos(3,3,-4));
      if(mode==1) { logs.add(new BlockPos(4,3,-4)); roots.add(new BlockPos(3,3,-3)); }
      if(mode==2) roots.add(new BlockPos(5,4,-4));
      if(mode==4) roots.add(new BlockPos(5,2,-4));
      Function<BlockPos,BlockState> at=p->{var found=blocks.get(p); if(found!=null)return found; if(p.getY()==2)return Blocks.STONE.defaultBlockState(); if(p.getY()==3&&Math.floorMod(p.getX()-p.getZ(),5)==0)return Blocks.VINE.defaultBlockState(); return Blocks.AIR.defaultBlockState();};
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(p,m,a)->{
        if(m.getName().equals("getBlockState"))return at.apply((BlockPos)a[0]);
        if(m.getName().equals("isStateAtPosition"))return ((Predicate<BlockState>)a[1]).test(at.apply((BlockPos)a[0]));
        if(m.getName().equals("getHeightmapPos")){var pos=(BlockPos)a[1];return new BlockPos(pos.getX(),Math.floorMod(pos.getX()*31+pos.getZ(),7)==0?6:3,pos.getZ());}
        throw new UnsupportedOperationException(m.toString());
      });
      BiConsumer<BlockPos,BlockState> setter=(p,s)->{p=p.immutable();blocks.put(p,s);writes.add(p.getX()+","+p.getY()+","+p.getZ()+","+s.getValue(LeafLitterBlock.FACING)+","+s.getValue(net.minecraft.world.level.block.SegmentableBlock.AMOUNT));};
      var litter=Blocks.LEAF_LITTER.defaultBlockState();
      var weighted=WeightedList.<BlockState>builder().add(litter,1).add(litter.setValue(LeafLitterBlock.FACING,net.minecraft.core.Direction.EAST).setValue(net.minecraft.world.level.block.SegmentableBlock.AMOUNT,3),2);
      var decorator=new PlaceOnGroundDecorator(30,2,1,Holder.direct(new WeightedStateProvider(weighted)));
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      decorator.place(new TreeDecorator.Context(world,setter,random,logs,Set.of(),roots));
      out.printf("G %d %d %s R %d%n",seed,mode,String.join(";",writes),random.nextLong());
    }
  }
}
