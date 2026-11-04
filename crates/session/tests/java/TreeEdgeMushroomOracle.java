import java.lang.reflect.Proxy;
import java.util.HashMap;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BooleanProperty;

public class TreeEdgeMushroomOracle {
  public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var level=(LevelReader)Proxy.newProxyInstance(LevelReader.class.getClassLoader(),new Class[]{LevelReader.class},(o,m,a)->{throw new AssertionError(m.toString());});
    var ticks=(ScheduledTickAccess)Proxy.newProxyInstance(ScheduledTickAccess.class.getClassLoader(),new Class[]{ScheduledTickAccess.class},(o,m,a)->{throw new AssertionError(m.toString());});
    var blocks=new Block[]{Blocks.RED_MUSHROOM_BLOCK,Blocks.BROWN_MUSHROOM_BLOCK,Blocks.MUSHROOM_STEM};
    var properties=new BooleanProperty[]{HugeMushroomBlock.DOWN,HugeMushroomBlock.UP,HugeMushroomBlock.NORTH,HugeMushroomBlock.SOUTH,HugeMushroomBlock.WEST,HugeMushroomBlock.EAST};
    var directions=new Direction[]{Direction.DOWN,Direction.UP,Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
    for(int type=0;type<blocks.length;type++) for(int mask=0;mask<64;mask++) {
      BlockState state=blocks[type].defaultBlockState();
      for(int i=0;i<6;i++)state=state.setValue(properties[i],(mask&(1<<i))!=0);
      if(state.hasBlockEntity() || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state) || state.getPostProcessPos(null,BlockPos.ZERO)!=null)throw new AssertionError("unsupported mushroom effects "+state);
      for(int dir=0;dir<6;dir++)out.printf("S %d %d %d %s %s%n",type,mask,dir,state.isFaceSturdy(null,BlockPos.ZERO,directions[dir]),state.isSolidRender());
      BlockState[] neighbors={Blocks.AIR.defaultBlockState(),Blocks.OAK_LOG.defaultBlockState(),blocks[type].defaultBlockState(),blocks[0].defaultBlockState(),blocks[1].defaultBlockState(),blocks[2].defaultBlockState()};
      for(int dir=0;dir<6;dir++)for(int n=0;n<neighbors.length;n++){
        var random=RandomSource.create(42);var pos=new BlockPos(8,8,8);
        var result=state.updateShape(level,ticks,pos,directions[dir],pos.relative(directions[dir]),neighbors[n],random);
        int after=0;for(int i=0;i<6;i++)if(result.getValue(properties[i]))after|=1<<i;
        if(random.nextLong()!=RandomSource.create(42).nextLong())throw new AssertionError("unexpected RNG use");
        out.printf("M %d %d %d %d %d%n",type,mask,dir,n,after);
      }
    }
    var shapeDirections=new Direction[]{Direction.WEST,Direction.EAST,Direction.NORTH,Direction.SOUTH,Direction.DOWN,Direction.UP};
    var origin=new BlockPos(8,8,8); var neighbors=new HashMap<BlockPos,BlockState>();
    var random=RandomSource.create(42);
    var accessor=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(o,m,a)->{
      if(m.getName().equals("getBlockState"))return neighbors.get(a[0]);
      if(m.getName().equals("getRandom"))return random;
      throw new AssertionError(m.toString());
    });
    int[] masks={0,1,5,21,42,63}; int[] adjacent={0,1,2,3,12,21,42,63};
    for(int type=0;type<blocks.length;type++)for(int mask:masks)for(int surround:adjacent){
      var initial=blocks[type].defaultBlockState();
      for(int i=0;i<6;i++)initial=initial.setValue(properties[i],(mask&(1<<i))!=0);
      neighbors.clear();for(int i=0;i<6;i++)neighbors.put(origin.relative(shapeDirections[i]),(surround&(1<<i))!=0?blocks[type].defaultBlockState():Blocks.AIR.defaultBlockState());
      var result=Block.updateFromNeighbourShapes(initial,accessor,origin);
      int after=0;for(int i=0;i<6;i++)if(result.getValue(properties[i]))after|=1<<i;
      out.printf("P %d %d %d %d%n",type,mask,surround,after);
    }
  }
}
