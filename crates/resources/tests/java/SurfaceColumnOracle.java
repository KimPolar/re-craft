import com.mojang.serialization.MapCodec;
import java.lang.reflect.*;
import java.util.Set;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.Interval;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.*;
import net.minecraft.world.level.levelgen.material.*;
import net.minecraft.world.level.levelgen.material.rule.*;
import net.minecraft.world.level.levelgen.synth.Noise;

public class SurfaceColumnOracle {
  static void set(Object o, String name, Object value) throws Exception {
    Field f=o.getClass().getDeclaredField(name); f.setAccessible(true); f.set(o,value);
  }
  static BlockState initial(int x,int y,int z) {
    int top=8+(x*3+z*7)%20;
    if(y>top) return y<15 ? Blocks.WATER.defaultBlockState() : Blocks.AIR.defaultBlockState();
    if(y>1 && (y+x*5+z*3)%11==0) return Blocks.AIR.defaultBlockState();
    if(y>1 && (y+x*7+z*5)%13==0) return Blocks.LAVA.defaultBlockState();
    return Blocks.STONE.defaultBlockState();
  }
  static class Chunk extends ProtoChunk {
    Chunk() { super(null,null,null,null,null); }
    @Override public ChunkPos getPos() { return new ChunkPos(0,0); }
    @Override public int getMinY() { return 0; }
    @Override public int getHeight() { return 32; }
    @Override public LevelHeightAccessor getHeightAccessorForGeneration() { return LevelHeightAccessor.create(0,32); }
    @Override public int getHighestFilledSectionIndex() { return 1; }
    @Override public int getHeight(Heightmap.Types type,int x,int z) {
      for(int y=31;y>=0;y--) if(!initial(x,y,z).isAir()) return y;
      return -1;
    }
    @Override public BlockState getBlockState(BlockPos p) {
      if(p.getY()<0 || p.getY()>=32) return Blocks.AIR.defaultBlockState();
      return initial(p.getX()&15,p.getY(),p.getZ()&15);
    }
    @Override public BlockState setBlockState(BlockPos p,BlockState state,int flags) { throw new AssertionError("rule must not replace blocks"); }
  }
  public static void main(String[] args) throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)f.get(null);
    var chunk=(Chunk)unsafe.allocateInstance(Chunk.class);
    var noiseChunk=(NoiseChunk)unsafe.allocateInstance(NoiseChunk.class);
    set(noiseChunk,"volume",new DensityVolume(16,32,16,0,0,0));
    var system=(MaterialSystem)unsafe.allocateInstance(MaterialSystem.class);
    Noise noise=new Noise() {
      public Interval range(){return null;}
      public float get(double x,double z){return 0;}
      public float get(double x,double y,double z){return 0;}
    };
    set(system,"surfaceNoise",noise);
    set(system,"noiseRandom",new XoroshiroRandomSource(1).forkPositional());
    var biome=new Biome.BiomeBuilder().temperature(0.8f).downfall(0.5f).hasPrecipitation(true)
      .specialEffects(new BiomeSpecialEffects.Builder().waterColor(0).build())
      .generationSettings(BiomeGenerationSettings.EMPTY).build();
    var holder=Holder.direct(biome);
    var manager=new BiomeManager((x,y,z)->holder,0);
    LevelAccessor level=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(o,m,a)->switch(m.getName()) {
      case "getMinY" -> 0; case "getHeight" -> 32; case "getSeaLevel" -> 15;
      default -> throw new UnsupportedOperationException(m.getName());
    });
    var ctor=WorldGenerationContext.class.getDeclaredConstructor(LevelAccessor.class); ctor.setAccessible(true);
    var world=ctor.newInstance(level);
    MaterialRule rule=new MaterialRule() {
      public MapCodec<? extends MaterialRule> codec(){return null;}
      public RuleEvaluator compile(MaterialRuleContext c) { return (x,y,z)->{
        out.printf("P %d %d %d %d %d %d %d %d%n",x,y,z,c.stoneDepthAbove(),c.stoneDepthBelow(),c.waterHeight(),c.surfaceGradientX(),c.surfaceGradientZ());
        return null;
      }; }
    };
    system.buildSurface(null,manager,world,chunk,noiseChunk,rule,Set.of(holder));
  }
}
