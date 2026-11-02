import java.lang.reflect.Proxy;
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
      BlockState[] neighbors={Blocks.AIR.defaultBlockState(),Blocks.OAK_LOG.defaultBlockState(),blocks[type].defaultBlockState(),blocks[0].defaultBlockState(),blocks[1].defaultBlockState(),blocks[2].defaultBlockState()};
      for(int dir=0;dir<6;dir++)for(int n=0;n<neighbors.length;n++){
        var random=RandomSource.create(42);var pos=new BlockPos(8,8,8);
        var result=state.updateShape(level,ticks,pos,directions[dir],pos.relative(directions[dir]),neighbors[n],random);
        int after=0;for(int i=0;i<6;i++)if(result.getValue(properties[i]))after|=1<<i;
        if(random.nextLong()!=RandomSource.create(42).nextLong())throw new AssertionError("unexpected RNG use");
        out.printf("M %d %d %d %d %d%n",type,mask,dir,n,after);
      }
    }
  }
}
