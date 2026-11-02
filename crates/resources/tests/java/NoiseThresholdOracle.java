import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.levelgen.feature.stateproviders.NoiseThresholdProvider;
public class NoiseThresholdOracle {
 public static void main(String[] args) {
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var noise=NormalNoise.builder().setBaseAmplitude(0.955388882960065).setBaseOctave(0).setOctaveCount(1).build();
  var p=new NoiseThresholdProvider(2345,noise,0.005f,-0.8f,0.33333334f,Blocks.DANDELION.defaultBlockState(),
    List.of(Blocks.ORANGE_TULIP.defaultBlockState(),Blocks.RED_TULIP.defaultBlockState(),Blocks.PINK_TULIP.defaultBlockState(),Blocks.WHITE_TULIP.defaultBlockState()),
    List.of(Blocks.POPPY.defaultBlockState(),Blocks.AZURE_BLUET.defaultBlockState(),Blocks.OXEYE_DAISY.defaultBlockState(),Blocks.CORNFLOWER.defaultBlockState()));
  boolean chunk=args.length>0 && args[0].equals("chunk");
  for(int x=chunk?0:-2048;x<=(chunk?15:2048);x+=chunk?1:137)for(int z=chunk?0:-2048;z<=(chunk?15:2048);z+=chunk?1:191)for(int y:chunk?new int[]{8}:new int[]{0,64})for(long seed:new long[]{0,1}) {
   var r=new WorldgenRandom(new XoroshiroRandomSource(seed));var s=p.getState(null,r,new BlockPos(x,y,z));
   out.printf("C %d %d %d %d %s %d%n",x,y,z,seed,BuiltInRegistries.BLOCK.getKey(s.getBlock()),r.nextLong());
  }
 }
}
