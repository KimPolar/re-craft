import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.feature.TreeFeature;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.phys.shapes.DiscreteVoxelShape;

public class TreeLeavesOracle {
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.OAK_LOG.builtInRegistryHolder(),List.of(BlockTags.PREVENTS_NEARBY_LEAF_DECAY));
    var method=TreeFeature.class.getDeclaredMethod("updateLeaves",LevelAccessor.class,BoundingBox.class,Set.class,Set.class,Set.class);method.setAccessible(true);
    for(int seed=0;seed<4;seed++)for(int mode=0;mode<4;mode++){
      var map=new HashMap<BlockPos,BlockState>();var writes=new ArrayList<String>();
      for(int x=-3;x<=3;x++)for(int y=1;y<=5;y++)for(int z=-3;z<=3;z++)if(Math.floorMod(x*7+y*13+z*17+seed,5)!=0){
        int d=mode==1?1+Math.floorMod(x*3+y*7+z*11+seed,7):7;
        map.put(new BlockPos(x,y,z),Blocks.OAK_LEAVES.defaultBlockState().setValue(BlockStateProperties.DISTANCE,d));
      }
      var logs=new HashSet<BlockPos>();
      for(var p:List.of(new BlockPos(0,0,0),new BlockPos(0,1,0),new BlockPos(0,2,0),new BlockPos(9,2,0))){logs.add(p);map.put(p,Blocks.OAK_LOG.defaultBlockState());}
      if(mode==3)logs.clear();
      if(mode==1)map.put(new BlockPos(2,3,2),Blocks.OAK_LOG.defaultBlockState());
      var decoration=new HashSet<BlockPos>();var roots=new HashSet<BlockPos>();
      if(mode==2){decoration.add(new BlockPos(1,2,0));roots.add(new BlockPos(-1,2,0));}
      var world=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(proxy,m,v)->{
        if(m.getName().equals("getBlockState"))return map.getOrDefault(v[0],Blocks.AIR.defaultBlockState());
        if(m.getName().equals("setBlock")){var p=((BlockPos)v[0]).immutable();var s=(BlockState)v[1];if((int)v[2]!=19)throw new AssertionError("flags");map.put(p,s);writes.add(p.getX()+","+p.getY()+","+p.getZ()+","+s.getValue(BlockStateProperties.DISTANCE));return true;}
        throw new UnsupportedOperationException(m.toString());
      });
      var box=new BoundingBox(-4,0,-4,4,6,4);
      var shape=(DiscreteVoxelShape)method.invoke(null,world,box,logs,decoration,roots);
      var occupied=new ArrayList<String>();var leaves=new ArrayList<String>();
      for(int x=-4;x<=4;x++)for(int y=0;y<=6;y++)for(int z=-4;z<=4;z++){
        var p=new BlockPos(x,y,z);if(shape.isFull(x+4,y,z+4))occupied.add(x+","+y+","+z);
        var state=map.getOrDefault(p,Blocks.AIR.defaultBlockState());
        if(state.is(Blocks.OAK_LEAVES))leaves.add(x+","+y+","+z+","+state.getValue(BlockStateProperties.DISTANCE));
      }
      out.printf("C %d %d W %s S %s D %s%n",seed,mode,String.join(";",writes),String.join(";",occupied),String.join(";",leaves));
    }
  }
}
