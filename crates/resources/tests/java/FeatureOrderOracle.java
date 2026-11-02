import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.biome.FeatureSorter;
import net.minecraft.world.level.levelgen.placement.PlacedFeature;

public class FeatureOrderOracle {
  public static void main(String[] args) throws Exception {
    var out = System.out;
    SharedConstants.tryDetectVersion();
    Bootstrap.bootStrap();
    JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    JsonArray output = new JsonArray();
    for (JsonElement c : cases) {
      Map<Integer, PlacedFeature> pool = new HashMap<>();
      IdentityHashMap<PlacedFeature, Integer> identities = new IdentityHashMap<>();
      List<List<HolderSet<PlacedFeature>>> biomes = new ArrayList<>();
      for (JsonElement b : c.getAsJsonArray()) {
        List<HolderSet<PlacedFeature>> steps = new ArrayList<>();
        for (JsonElement s : b.getAsJsonArray()) {
          List<Holder<PlacedFeature>> holders = new ArrayList<>();
          for (JsonElement value : s.getAsJsonArray()) {
            int id = value.getAsInt();
            PlacedFeature f = pool.computeIfAbsent(id, k -> {
              PlacedFeature made = new PlacedFeature(null, List.of());
              identities.put(made, k);
              return made;
            });
            holders.add(Holder.direct(f));
          }
          steps.add(HolderSet.direct(holders));
        }
        biomes.add(steps);
      }
      try {
        var result = FeatureSorter.buildFeaturesPerStep(biomes, x -> x, false);
        JsonArray all = new JsonArray();
        for (var step : result) {
          JsonArray list = new JsonArray();
          for (PlacedFeature f : step.features()) list.add(identities.get(f));
          all.add(list);
        }
        output.add(all);
      } catch (IllegalStateException e) {
        output.add("cycle");
      }
    }
    out.println(output);
  }
}
