import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.biome.FeatureSorter;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.placement.PlacedFeature;

public class DecorationPlanOracle {
  public static void main(String[] args) throws Exception {
    var out = System.out;
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
    var pool = new HashMap<Integer,PlacedFeature>();
    var ids = new IdentityHashMap<PlacedFeature,Integer>();
    List<List<HolderSet<PlacedFeature>>> biomes = new ArrayList<>();
    for (var b: input.getAsJsonArray("biomes")) {
      var steps = new ArrayList<HolderSet<PlacedFeature>>();
      for (var step: b.getAsJsonArray()) {
        var holders = new ArrayList<Holder<PlacedFeature>>();
        for (var value: step.getAsJsonArray()) {
          int id = value.getAsInt();
          var f = pool.computeIfAbsent(id,k->{ var made=new PlacedFeature(null,List.of()); ids.put(made,k); return made; });
          holders.add(Holder.direct(f));
        }
        steps.add(HolderSet.direct(holders));
      }
      biomes.add(steps);
    }
    var schedule=FeatureSorter.buildFeaturesPerStep(biomes,x->x,false);
    JsonArray output=new JsonArray();
    for(var item:input.getAsJsonArray("cases")) {
      var c=item.getAsJsonObject();
      var rng=new WorldgenRandom(new XoroshiroRandomSource(0));
      long deco=rng.setDecorationSeed(c.get("seed").getAsLong(),c.get("x").getAsInt()*16,c.get("z").getAsInt()*16);
      var result=new JsonObject(); result.addProperty("deco",deco);
      var resultFeatures=new JsonArray();
      for(int s=0;s<schedule.size();s++) {
        var indices=new TreeSet<Integer>();
        for(var b:c.getAsJsonArray("present")) {
          int biome=b.getAsInt();
          if(biome<0||biome>=biomes.size()||s>=biomes.get(biome).size()) continue;
          for(var holder:biomes.get(biome).get(s)) indices.add(schedule.get(s).indexMapping().applyAsInt(holder.value()));
        }
        for(int index:indices) {
          rng.setFeatureSeed(deco,index,s);
          var row=new JsonArray(); row.add(s); row.add(index); row.add(ids.get(schedule.get(s).features().get(index))); row.add(rng.nextLong());
          resultFeatures.add(row);
        }
      }
      result.add("features",resultFeatures); output.add(result);
    }
    out.println(output);
  }
}
