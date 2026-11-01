import com.mojang.serialization.MapCodec;
import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.Interval;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.*;
import net.minecraft.world.level.levelgen.material.*;
import net.minecraft.world.level.levelgen.material.rule.*;
import net.minecraft.world.level.levelgen.synth.Noise;

public class CarvedSurfaceOracle {
  static void set(Object o,String name,Object value)throws Exception {var f=o.getClass().getDeclaredField(name);f.setAccessible(true);f.set(o,value);}
  static int idx(int x,int y,int z){return (z*16+x)*16+y;}
  static class Chunk extends ProtoChunk {
    BlockState[] blocks;boolean[] updates;
    Chunk(){super(null,null,null,null,null);}
    public ChunkPos getPos(){return new ChunkPos(0,0);}
    public BlockState getBlockState(BlockPos p){return p.getY()<0||p.getY()>=16?Blocks.AIR.defaultBlockState():blocks[idx(p.getX()&15,p.getY(),p.getZ()&15)];}
    public BlockState setBlockState(BlockPos p,BlockState s,int flags){int i=idx(p.getX()&15,p.getY(),p.getZ()&15);var old=blocks[i];blocks[i]=s;return old;}
    public void markPosForPostProcessing(BlockPos p){updates[idx(p.getX()&15,p.getY(),p.getZ()&15)]=true;}
    public int getHeight(Heightmap.Types type,int x,int z){for(int y=15;y>=0;y--)if(!blocks[idx(x,y,z)].isAir())return y;return -1;}
  }
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);bind.invoke(Blocks.BEDROCK.builtInRegistryHolder(),List.of(BlockTags.UNCARVABLE));
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var chunk=(Chunk)unsafe.allocateInstance(Chunk.class);chunk.blocks=new BlockState[4096];chunk.updates=new boolean[4096];
    for(int z=0;z<16;z++)for(int x=0;x<16;x++)for(int y=0;y<16;y++)chunk.blocks[idx(x,y,z)]=(y<=3?Blocks.STONE:Blocks.AIR).defaultBlockState();
    for(int y=4;y<=15;y++)chunk.blocks[idx(3,y,4)]=Blocks.STONE.defaultBlockState();
    for(int[] p:new int[][]{{4,12,4},{6,10,4},{4,7,6}}){chunk.blocks[idx(p[0],p[1],p[2])]=Blocks.GRASS_BLOCK.defaultBlockState();chunk.blocks[idx(p[0],p[1]-1,p[2])]=Blocks.DIRT.defaultBlockState();}
    var mask=new CarvingMask(1,14);for(int[] p:new int[][]{{4,12,4},{6,10,4},{4,7,6}})mask.carve(p[0],p[1],p[2]);
    var system=(MaterialSystem)unsafe.allocateInstance(MaterialSystem.class);
    Noise zero=new Noise(){public Interval range(){return null;}public float get(double x,double z){return 0;}public float get(double x,double y,double z){return 0;}};
    set(system,"surfaceNoise",zero);set(system,"noiseRandom",new XoroshiroRandomSource(42).forkPositional());
    var preliminary=DensityFunctions.constant(12);set(system,"preliminarySurfaceFunction",preliminary);set(system,"seaLevel",8);
    var compiler=new DensityFunctionCompiler(null);DensitySamplerSet samplers=d -> new DensitySampler.Bound(compiler.getSampler(d),SamplerContext.EMPTY_UNCACHED);
    var noise=(NoiseChunk)unsafe.allocateInstance(NoiseChunk.class);set(noise,"cachingSamplers",samplers);
    Aquifer.FluidPicker picker=(x,y,z)->y < -54 ? new Aquifer.FluidStatus(-54,Blocks.LAVA.defaultBlockState()):new Aquifer.FluidStatus(8,Blocks.WATER.defaultBlockState());
    set(noise,"aquifer",Aquifer.createDisabled(picker));
    var random=(RandomState)unsafe.allocateInstance(RandomState.class);set(random,"materialSystem",system);
    LevelAccessor level=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(o,m,a)->switch(m.getName()){case "getMinY"->0;case "getHeight"->16;case "getSeaLevel"->8;default->throw new UnsupportedOperationException(m.getName());});
    var ctor=WorldGenerationContext.class.getDeclaredConstructor(LevelAccessor.class);ctor.setAccessible(true);var context=ctor.newInstance(level);
    MaterialRule rule=new MaterialRule(){public MapCodec<? extends MaterialRule> codec(){return null;}public RuleEvaluator compile(MaterialRuleContext c){return (x,y,z)->{
      if(c.stoneDepthAbove()!=1||c.stoneDepthBelow()!=1)throw new AssertionError("depth");
      if(c.waterHeight()!=Integer.MIN_VALUE)return Blocks.CLAY.defaultBlockState();
      if(c.surfaceGradientX()<=-4||c.surfaceGradientZ()>=4)return Blocks.PODZOL.defaultBlockState();
      return (y>=c.getMinSurfaceLevel()?Blocks.GRASS_BLOCK:Blocks.STONE).defaultBlockState();};}};
    var generator=(NoiseBasedChunkGenerator)unsafe.allocateInstance(NoiseBasedChunkGenerator.class);
    Method method=Arrays.stream(NoiseBasedChunkGenerator.class.getDeclaredMethods()).filter(m->m.getName().equals("applyCarvingMask")).findFirst().orElseThrow();method.setAccessible(true);
    method.invoke(generator,chunk,mask,random,rule,context,noise,null,null);
    for(int[] p:new int[][]{{4,12,4},{4,11,4},{6,10,4},{6,9,4},{4,7,6},{4,6,6}}){int i=idx(p[0],p[1],p[2]);out.printf("P %d %d %d %s %s%n",p[0],p[1],p[2],chunk.blocks[i].getBlock().builtInRegistryHolder().key().identifier(),chunk.updates[i]);}
    for(var block:BuiltInRegistries.BLOCK)out.println("R "+BuiltInRegistries.BLOCK.getKey(block));
  }
}
