import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.HugeMushroomBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.*;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

public class HugeMushroomOracle {
  public static void main(String[] args) throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
    bind.invoke(Blocks.OAK_LEAVES.builtInRegistryHolder(),List.of(BlockTags.LEAVES,BlockTags.REPLACEABLE_BY_MUSHROOMS));
    bind.invoke(Blocks.LEAF_LITTER.builtInRegistryHolder(),List.of(BlockTags.REPLACEABLE_BY_MUSHROOMS));
    for(boolean red:new boolean[]{false,true}) for(long seed:new long[]{0,1,-1,987654321}) for(int mode=0;mode<6;mode++) {
      var blocks=new HashMap<BlockPos,BlockState>(); var writes=new ArrayList<String>();
      if(mode!=1) blocks.put(new BlockPos(3,2,-4),Blocks.DIRT.defaultBlockState());
      if(mode==2) blocks.put(new BlockPos(3,4,-4),Blocks.STONE.defaultBlockState());
      if(mode==3) blocks.put(new BlockPos(4,7,-4),Blocks.OAK_LEAVES.defaultBlockState());
      if(mode==4) blocks.put(new BlockPos(4,7,-4),Blocks.STONE.defaultBlockState());
      if(mode==5) blocks.put(new BlockPos(4,7,-4),Blocks.LEAF_LITTER.defaultBlockState());
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(p,m,a)->{
        if(m.getName().equals("getMinY")) return 0;
        if(m.getName().equals("getMaxY")) return 63;
        if(m.getName().equals("getBlockState")) return blocks.getOrDefault(a[0],Blocks.AIR.defaultBlockState());
        if(m.getName().equals("setBlockAndUpdate")) {
          var pos=((BlockPos)a[0]).immutable(); var state=(BlockState)a[1]; blocks.put(pos,state);
          String faces=state.is(Blocks.MUSHROOM_STEM)?"-":String.format("%s,%s,%s,%s,%s,%s",state.getValue(HugeMushroomBlock.WEST),state.getValue(HugeMushroomBlock.EAST),state.getValue(HugeMushroomBlock.NORTH),state.getValue(HugeMushroomBlock.SOUTH),state.getValue(HugeMushroomBlock.UP),state.getValue(HugeMushroomBlock.DOWN));
          writes.add(pos.getX()+","+pos.getY()+","+pos.getZ()+","+(state.is(Blocks.MUSHROOM_STEM)?"stem":"cap")+","+faces); return true;
        }
        throw new UnsupportedOperationException(m.toString());
      });
      var cap=BlockStateProvider.holderOf(red?Blocks.RED_MUSHROOM_BLOCK:Blocks.BROWN_MUSHROOM_BLOCK);
      var stem=BlockStateProvider.holderOf(Blocks.MUSHROOM_STEM);
      var ground=BlockPredicate.matchesBlocks(Blocks.DIRT);
      AbstractHugeMushroomFeature feature=red?new HugeRedMushroomFeature(cap,stem,3,ground):new HugeBrownMushroomFeature(cap,stem,3,ground);
      var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      boolean result=feature.place(world,null,random,new BlockPos(3,3,-4));
      out.printf("M %s %d %d %s %s R %d%n",red,seed,mode,result,String.join(";",writes),random.nextLong());
    }
  }
}
