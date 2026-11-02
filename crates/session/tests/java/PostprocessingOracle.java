import java.lang.reflect.*;
import java.util.*;
import it.unimi.dsi.fastutil.shorts.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.*;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.material.*;

public class PostprocessingOracle {
  static java.io.PrintStream output;
  static final Map<BlockPos,TraceState> blocks=new HashMap<>();
  static TraceState a,b,c,d;
  static int shapeCount;
  static sun.misc.Unsafe unsafe;
  static void set(Object object,String name,Object value)throws Exception {
    Class<?> type=object.getClass();
    while(type!=null) { try {var field=type.getDeclaredField(name);field.setAccessible(true);field.set(object,value);return;}catch(NoSuchFieldException e){type=type.getSuperclass();} }
    throw new NoSuchFieldException(name);
  }
  static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
  static class FakeFluid extends EmptyFluid {
    @Override protected boolean isEmpty(){return false;}
    @Override protected void tick(ServerLevel level,BlockPos p,BlockState state,FluidState fluid) {
      var original=(TraceState)state;
      output.println("F "+pos(p)+" "+original.name);
      blocks.put(p.immutable(),c);
    }
  }
  static class TraceState extends BlockState {
    String name; boolean wet,liquid,change;
    static FluidState fluid;
    TraceState(){super(Blocks.STONE,new Property<?>[0],new Comparable<?>[0]);}
    @Override public FluidState getFluidState(){return wet?fluid:Fluids.EMPTY.defaultFluidState();}
    @Override public Block getBlock(){return liquid?Blocks.WATER:Blocks.STONE;}
    @Override public void tick(ServerLevel level,BlockPos p,RandomSource random){
      output.println("L "+pos(p)+" "+name);blocks.put(p.immutable(),b);
    }
    @Override public BlockState updateShape(LevelReader level,ScheduledTickAccess ticks,BlockPos p,Direction direction,BlockPos neighbour,BlockState other,RandomSource random) {
      if(shapeCount++%6==0)output.println("S "+pos(p)+" "+name);
      return change?d:this;
    }
  }
  static class FakeLevel extends ServerLevel {
    FakeLevel(){super(null,null,null,null,null,null,false,0L,List.of(),false);}
    @Override public BlockState getBlockState(BlockPos pos){return Blocks.AIR.defaultBlockState();}
    @Override public RandomSource getRandom(){return RandomSource.create(3);}
    @Override public boolean setBlock(BlockPos p,BlockState state,int flags,int recursion){
      var result=(TraceState)state;output.println("W "+pos(p)+" "+result.name+" "+flags);blocks.put(p.immutable(),result);return true;
    }
  }
  static class FakeChunk extends LevelChunk {
    FakeChunk(){super((Level)null,new ChunkPos(-1,2));}
    @Override public ChunkPos getPos(){return new ChunkPos(-1,2);}
    @Override public int getMinY(){return -16;}
    @Override public int getHeight(){return 32;}
    @Override public BlockState getBlockState(BlockPos p){return blocks.get(p);}
  }
  static class FakeUpgrade extends UpgradeData {
    FakeUpgrade(){super(new CompoundTag(),LevelHeightAccessor.create(-16,32));}
    @Override public void upgrade(LevelChunk chunk){}
  }
  static TraceState state(String name,boolean wet,boolean liquid,boolean change)throws Exception {
    var s=(TraceState)unsafe.allocateInstance(TraceState.class);s.name=name;s.wet=wet;s.liquid=liquid;s.change=change;return s;
  }
  public static void main(String[] args)throws Exception {
    output=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);unsafe=(sun.misc.Unsafe)f.get(null);
    var fluid=(FakeFluid)unsafe.allocateInstance(FakeFluid.class);
    TraceState.fluid=new FluidState(fluid,new Property<?>[0],new Comparable<?>[0]);
    a=state("a",true,true,false);b=state("b",true,false,true);c=state("c",false,false,false);d=state("d",false,false,false);
    var level=(FakeLevel)unsafe.allocateInstance(FakeLevel.class);
    var chunk=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
    var p1=new BlockPos(-16,-15,32);var p2=new BlockPos(-15,0,32);var p3=new BlockPos(-14,0,32);
    blocks.put(p1,a);blocks.put(p2,b);blocks.put(p3,c);
    ShortList[] lists={new ShortArrayList(),new ShortArrayList()};
    lists[0].add(ProtoChunk.packOffsetCoordinates(p1));lists[0].add(ProtoChunk.packOffsetCoordinates(p1));
    lists[1].add(ProtoChunk.packOffsetCoordinates(p2));lists[1].add(ProtoChunk.packOffsetCoordinates(p3));
    set(chunk,"postProcessing",lists);set(chunk,"pendingBlockEntities",new HashMap<>());
    set(chunk,"upgradeData",unsafe.allocateInstance(FakeUpgrade.class));
    chunk.postProcessGeneration(level);
    output.println("E "+lists[0].size()+" "+lists[1].size());
  }
}
