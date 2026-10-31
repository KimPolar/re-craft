import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.levelgen.densityfunction.*;

public class EndBiomeSourceOracle {
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var data=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
    Holder<Biome>[] holders=new Holder[5];
    for(int i=0;i<holders.length;i++) {
      var biome=new Biome.BiomeBuilder().temperature((float)i/10).downfall(0.5f).hasPrecipitation(false)
        .specialEffects(new BiomeSpecialEffects.Builder().waterColor(0).build())
        .generationSettings(BiomeGenerationSettings.EMPTY).build();
      holders[i]=Holder.direct(biome);
    }
    var ctor=TheEndBiomeSource.class.getDeclaredConstructors()[0];ctor.setAccessible(true);
    var source=(TheEndBiomeSource)ctor.newInstance(holders[0],holders[1],holders[2],holders[3],holders[4]);
    String[] names={"minecraft:the_end","minecraft:end_highlands","minecraft:end_midlands","minecraft:small_end_islands","minecraft:end_barrens"};
    for(var expression:data.getAsJsonArray("erosion")) {
      var function=DensityFunctions.DIRECT_CODEC.parse(JsonOps.INSTANCE,expression).getOrThrow();
      var compiler=new DensityFunctionCompiler(null);
      var bound=new DensitySampler.Bound(compiler.getSampler(function),SamplerContext.EMPTY_UNCACHED);
      var sampler=new Climate.Sampler(bound,bound,bound,bound,bound,bound);
      var resolver=source.createResolverForChunk(sampler,0,0,0,4,32,4);
      out.println("S");
      for(var point:data.getAsJsonArray("points")) {
        var p=point.getAsJsonArray();var result=resolver.getNoiseBiome(p.get(0).getAsInt(),p.get(1).getAsInt(),p.get(2).getAsInt());
        int index=0;while(index<5 && result!=holders[index])index++;
        if(index==5)throw new AssertionError(result);
        out.println("P "+names[index]);
      }
    }
  }
}
