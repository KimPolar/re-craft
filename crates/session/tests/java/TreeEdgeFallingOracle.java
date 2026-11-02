import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.material.Fluid;

public class TreeEdgeFallingOracle {
  public static void main(String[] args) {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var level=(LevelReader)Proxy.newProxyInstance(LevelReader.class.getClassLoader(),new Class[]{LevelReader.class},(o,m,a)->{throw new AssertionError(m.toString());});
    var blocks=new Block[]{Blocks.SAND,Blocks.RED_SAND,Blocks.GRAVEL};
    var neighbors=new Block[]{Blocks.AIR,Blocks.STONE,Blocks.OAK_LOG};
    for(int kind=0;kind<blocks.length;kind++) for(Direction dir:Direction.values()) for(int n=0;n<neighbors.length;n++) {
      var events=new ArrayList<String>();
      var ticks=(ScheduledTickAccess)Proxy.newProxyInstance(ScheduledTickAccess.class.getClassLoader(),new Class[]{ScheduledTickAccess.class},(o,m,a)->{
        if(m.getName().equals("scheduleTick")){if(!new BlockPos(8,8,8).equals(a[0]))throw new AssertionError("unexpected tick position");events.add((a[1] instanceof Fluid ? "F:"+BuiltInRegistries.FLUID.getKey((Fluid)a[1]):"B:"+BuiltInRegistries.BLOCK.getKey((Block)a[1]))+":"+a[2]);return null;}
        throw new AssertionError(m.toString());
      });
      var state=blocks[kind].defaultBlockState(); var pos=new BlockPos(8,8,8); var random=RandomSource.create(42);
      var updated=state.updateShape(level,ticks,pos,dir,pos.relative(dir),neighbors[n].defaultBlockState(),random);
      if(random.nextLong()!=RandomSource.create(42).nextLong())throw new AssertionError("unexpected RNG use");
      out.printf("F %d %s %d %s %s%n",kind,dir.getSerializedName(),n,updated==state,String.join(",",events));
    }
  }
}
