import com.google.gson.JsonParser;
import com.mojang.datafixers.util.Pair;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList;

public class BiomeSourceOracle {
  public static void main(String[] args)throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var document=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
    for(var group:document.getAsJsonArray("groups")) {
      var object=group.getAsJsonObject();
      Climate.ParameterList<String> parameters;
      if(object.has("nether")) {
        var preset=MultiNoiseBiomeSourceParameterList.knownPresets().get(MultiNoiseBiomeSourceParameterList.Preset.NETHER);
        var entries=new ArrayList<Pair<Climate.ParameterPoint,String>>();
        for(var entry:preset.values()) entries.add(Pair.of(entry.getFirst(),entry.getSecond().identifier().toString()));
        parameters=new Climate.ParameterList<>(entries);
      } else {
        var entries=new ArrayList<Pair<Climate.ParameterPoint,String>>();
        for(var entry:object.getAsJsonArray("biomes")) {
          var e=entry.getAsJsonObject();
          entries.add(Pair.of(Climate.ParameterPoint.CODEC.parse(JsonOps.INSTANCE,e.get("parameters")).getOrThrow(), e.get("biome").getAsString()));
        }
        parameters=new Climate.ParameterList<>(entries);
      }
      out.println("S");
      for(var entry:document.getAsJsonArray("targets")) {
        var t=entry.getAsJsonArray();
        var target=Climate.target(t.get(0).getAsFloat(),t.get(1).getAsFloat(),t.get(2).getAsFloat(),t.get(3).getAsFloat(),t.get(4).getAsFloat(),t.get(5).getAsFloat());
        out.println("P "+parameters.findValue(target));
      }
    }
  }
}
