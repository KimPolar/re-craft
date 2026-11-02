import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.feature.VinesFeature;

public class VinesFeatureOracle {
  public static void main(String[] args) {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var faces=new Direction[]{Direction.UP,Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
    var origin=new BlockPos(2,5,-3);
    for(int kind=0;kind<7;kind++)for(int mask=0;mask<32;mask++)for(boolean occupied:new boolean[]{false,true}) {
      final int k=kind, bits=mask; var writes=new ArrayList<String>();
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,method,v)->{
        if(method.getName().equals("isEmptyBlock"))return !occupied;
        if(method.getName().equals("getBlockState")) {
          var p=(BlockPos)v[0];
          for(int i=0;i<faces.length;i++) if(p.equals(origin.relative(faces[i])) && (bits&(1<<i))!=0) {
            if(k==0)return Blocks.STONE.defaultBlockState();
            if(k==1)return Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.BOTTOM);
            if(k==2)return Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.TOP);
            if(k==3)return Blocks.GLASS.defaultBlockState();
            if(k==4)return Blocks.OAK_LOG.defaultBlockState();
            if(k==5)return Blocks.OAK_LEAVES.defaultBlockState();
            return Blocks.VINE.defaultBlockState();
          }
          return Blocks.AIR.defaultBlockState();
        }
        if(method.getName().equals("setBlock")) {
          var state=(BlockState)v[1];if((int)v[2]!=2)throw new AssertionError();
          writes.add(state.getValues().map(x->x.property().getName()+"="+x.valueName()).sorted().reduce((a,b)->a+","+b).orElse(""));return true;
        }
        throw new UnsupportedOperationException(method.toString());
      });
      var random=new WorldgenRandom(new XoroshiroRandomSource(42));
      boolean result=new VinesFeature().place(world,null,random,origin);
      out.printf("V %d %d %s %s %s R %d%n",kind,mask,occupied,result,String.join(";",writes),random.nextLong());
    }
  }
}
