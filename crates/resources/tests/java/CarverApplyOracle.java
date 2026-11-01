import java.lang.reflect.*;
import java.util.*;
import java.util.function.Function;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.DensitySamplerSet;
import net.minecraft.world.level.levelgen.material.MaterialSystem;
import net.minecraft.world.level.levelgen.material.rule.MaterialRule;

public class CarverApplyOracle {
  static void set(Object o,String name,Object value)throws Exception { var f=o.getClass().getDeclaredField(name);f.setAccessible(true);f.set(o,value); }
  static class Chunk extends ProtoChunk {
    BlockState[] blocks; boolean[] updates;
    Chunk(){super(null,null,null,null,null);}
    public ChunkPos getPos(){return new ChunkPos(0,0);}
    public BlockState getBlockState(BlockPos p){return p.getY()<0 || p.getY()>=16?Blocks.AIR.defaultBlockState():blocks[p.getY()];}
    public BlockState setBlockState(BlockPos p,BlockState s,int flags){var old=blocks[p.getY()];blocks[p.getY()]=s;return old;}
    public void markPosForPostProcessing(BlockPos p){updates[p.getY()]=true;}
  }
  static class Material extends MaterialSystem {
    Material(){super(null,null,0,null,null);}
    @Override public Optional<BlockState> topMaterial(MaterialRule rule,RandomState random,WorldGenerationContext context,
      Function<BlockPos,Holder<Biome>> biome,ChunkAccess chunk,DensitySamplerSet samplers,BlockPos p,boolean fluid){
      System.out.printf("T %d %s%n",p.getY(),fluid);
      return Optional.of((fluid?Blocks.WATER:Blocks.GRASS_BLOCK).defaultBlockState());
    }
  }
  public static void main(String[] args)throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    bind.invoke(Blocks.BEDROCK.builtInRegistryHolder(),List.of(BlockTags.UNCARVABLE));
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);var unsafe=(sun.misc.Unsafe)f.get(null);
    var chunk=(Chunk)unsafe.allocateInstance(Chunk.class);chunk.blocks=new BlockState[16];chunk.updates=new boolean[16];
    Arrays.fill(chunk.blocks,Blocks.STONE.defaultBlockState());
    chunk.blocks[2]=Blocks.DIRT.defaultBlockState();chunk.blocks[6]=Blocks.DIRT.defaultBlockState();
    chunk.blocks[7]=Blocks.GRASS_BLOCK.defaultBlockState();chunk.blocks[8]=Blocks.BEDROCK.defaultBlockState();chunk.blocks[10]=Blocks.DIRT.defaultBlockState();
    var mask=new CarvingMask(1,14);for(int y:new int[]{3,4,5,7,8,11})mask.carve(0,y,0);
    Aquifer aquifer=new Aquifer(){public BlockState computeSubstance(int x,int y,int z,double density){return y==4?null:(y==7?Blocks.WATER:Blocks.CAVE_AIR).defaultBlockState();}public boolean shouldScheduleFluidUpdate(){return true;}};
    var noise=(NoiseChunk)unsafe.allocateInstance(NoiseChunk.class);set(noise,"aquifer",aquifer);
    var random=(RandomState)unsafe.allocateInstance(RandomState.class);set(random,"materialSystem",unsafe.allocateInstance(Material.class));
    var generator=(NoiseBasedChunkGenerator)unsafe.allocateInstance(NoiseBasedChunkGenerator.class);
    Method method=Arrays.stream(NoiseBasedChunkGenerator.class.getDeclaredMethods()).filter(m->m.getName().equals("applyCarvingMask")).findFirst().orElseThrow();method.setAccessible(true);
    CarvingMask.Filter filter=(x,y,z)->y!=5;
    method.invoke(generator,chunk,mask,random,null,null,noise,null,filter);
    for(int y=0;y<16;y++)out.printf("P %d %s %s%n",y,chunk.blocks[y].getBlock().builtInRegistryHolder().key().identifier(),chunk.updates[y]);
  }
}
