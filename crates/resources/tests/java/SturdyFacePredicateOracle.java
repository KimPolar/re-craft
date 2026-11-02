import java.lang.reflect.Proxy;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Vec3i;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.levelgen.blockpredicates.HasSturdyFacePredicate;

public class SturdyFacePredicateOracle {
  public static void main(String[] args){
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    for(String kind:new String[]{"air","stone","bottom","top","double"})for(Direction direction:Direction.values()){
      BlockState state=switch(kind){case "air"->Blocks.AIR.defaultBlockState();case "stone"->Blocks.STONE.defaultBlockState();default->Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,switch(kind){case "bottom"->SlabType.BOTTOM;case "top"->SlabType.TOP;default->SlabType.DOUBLE;});};
      var world=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(p,m,v)->{
        if(m.getName().equals("getBlockState")){if(!v[0].equals(new BlockPos(4,6,5)))throw new AssertionError(v[0]);return state;}
        throw new UnsupportedOperationException(m.toString());
      });
      boolean result=new HasSturdyFacePredicate(new Vec3i(1,-2,3),direction).test(world,new BlockPos(3,8,2));
      out.printf("S %s %s %s%n",kind,direction.getSerializedName(),result);
    }
  }
}
