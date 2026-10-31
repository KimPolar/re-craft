import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.*;
import net.minecraft.world.level.levelgen.synth.Noise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public class SteppedDensityOracle {
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var json=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
    for(int mode=0;mode<2;mode++) {
      long seed=2345;
      RandomSource root=mode==0?new LegacyRandomSource(seed):new XoroshiroRandomSource(seed);
      var factory=root.forkPositional();
      var ctx=new DensityFunction.CompileContext() {
        public Noise createNoiseSampler(Holder<NormalNoise> noise) {return noise.value().create(factory.fromHashOf(Identifier.withDefaultNamespace("surface")));}
        public RandomSource createRandom(Identifier name) {return factory.fromHashOf(name);}
        public RandomSource createEndIslandRandom(){return new LegacyRandomSource(seed);}
      };
      var compiler=new DensityFunctionCompiler(ctx);
      for(var fixture:json.getAsJsonArray("fixtures")) {
        var function=DensityFunctions.DIRECT_CODEC.parse(JsonOps.INSTANCE,fixture).getOrThrow();
        var sampler=compiler.getSampler(function);
        for(var test:json.getAsJsonArray("volumes")) {
          var v=test.getAsJsonObject();var min=v.getAsJsonArray("min");var size=v.getAsJsonArray("size");var step=v.getAsJsonArray("step");
          var volume=new DensityVolume(size.get(0).getAsInt(),size.get(1).getAsInt(),size.get(2).getAsInt(),min.get(0).getAsInt(),min.get(1).getAsInt(),min.get(2).getAsInt(),step.get(0).getAsInt(),step.get(1).getAsInt(),step.get(2).getAsInt());
          var buffer=DensityBuffer.createUnpooled(volume.size());
          sampler.sampleVolume(SamplerContext.EMPTY_UNCACHED,buffer,volume);
          out.println("S");
          for(int i=0;i<buffer.size();i++) out.printf("P %d%n",Float.floatToRawIntBits(buffer.get(i)));
        }
      }
    }
  }
}
