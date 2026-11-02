import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.ScheduledTickAccess;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.StateDefinition;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.phys.shapes.BitSetDiscreteVoxelShape;

public class TreeEdgeOracle {
  static final List<String> events=new ArrayList<>();
  static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
  static class Probe extends Block {
    Probe(){super(BlockBehaviour.Properties.of().setId(ResourceKey.create(Registries.BLOCK,Identifier.parse("test:probe"))));}
    @Override protected void createBlockStateDefinition(StateDefinition.Builder<Block,BlockState> b){b.add(BlockStateProperties.POWER);}
    @Override protected BlockState updateShape(BlockState state,LevelReader world,ScheduledTickAccess ticks,BlockPos p,Direction d,BlockPos n,BlockState other,RandomSource random){
      int value=state.getValue(BlockStateProperties.POWER),neighbor=other.getValue(BlockStateProperties.POWER),draw=random.nextInt(5);
      events.add("U"+pos(p)+","+d.getStepX()+","+d.getStepY()+","+d.getStepZ()+","+value+","+neighbor+","+draw);
      return state.setValue(BlockStateProperties.POWER,(value*3+neighbor+d.ordinal()+draw)%16);
    }
  }
  public static void main(String[] args)throws Exception{
    var out=System.out;SharedConstants.tryDetectVersion();
    var started=Bootstrap.class.getDeclaredField("isBootstrapped");started.setAccessible(true);started.setBoolean(null,true);
    var block=Registry.register(BuiltInRegistries.BLOCK,Identifier.parse("test:probe"),new Probe());
    started.setBoolean(null,false);
    Bootstrap.bootStrap();
    for(long seed:new long[]{0,1,-1,987654321})for(int mode=0;mode<5;mode++){
      events.clear();var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
      var map=new HashMap<BlockPos,BlockState>();
      var world=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(proxy,m,v)->{
        if(m.getName().equals("getRandom"))return random;
        if(m.getName().equals("getBlockState")){var p=(BlockPos)v[0];return map.getOrDefault(p,block.defaultBlockState().setValue(BlockStateProperties.POWER,Math.floorMod(p.getX()*3+p.getY()*5+p.getZ()*7,16)));}
        if(m.getName().equals("setBlock")){var p=((BlockPos)v[0]).immutable();var s=(BlockState)v[1];map.put(p,s);events.add("S"+pos(p)+","+s.getValue(BlockStateProperties.POWER)+","+v[2]);return true;}
        throw new UnsupportedOperationException(m.toString());
      });
      var shape=new BitSetDiscreteVoxelShape(4,3,5);
      for(int x=0;x<4;x++)for(int y=0;y<3;y++)for(int z=0;z<5;z++){
        boolean full=switch(mode){case 0->false;case 1->true;case 2->x==1&&y==1&&z==2;case 3->Math.floorMod(x*3+y*5+z*7,4)==0;default->(x+y+z)%2==0;};
        if(full)shape.fill(x,y,z);
      }
      StructureTemplate.updateShapeAtEdge(world,3,shape,-3,5,7);
      out.printf("C %d %d %s R %d%n",seed,mode,String.join(";",events),random.nextLong());
    }
  }
}
