import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.Constructor;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.world.level.biome.BiomeGenerationSettings;
import net.minecraft.world.level.levelgen.heightproviders.HeightProvider;
import net.minecraft.world.level.levelgen.placement.HeightRangePlacement;
import net.minecraft.world.level.levelgen.placement.PlacedFeature;

public class BiomeFeatureEqualityOracle {
  static BiomeGenerationSettings settings(PlacedFeature feature) throws Exception {
    Constructor<BiomeGenerationSettings> ctor = BiomeGenerationSettings.class.getDeclaredConstructor(HolderSet.class,List.class);
    ctor.setAccessible(true);
    return ctor.newInstance(HolderSet.empty(),List.of(HolderSet.direct(Holder.direct(feature))));
  }
  static HeightProvider height(String json) {
    return HeightProvider.CODEC.parse(JsonOps.INSTANCE,JsonParser.parseString(json)).getOrThrow();
  }
  static void compare(java.io.PrintStream out, String label, String json) throws Exception {
    var a = new PlacedFeature(null,List.of(new HeightRangePlacement(height(json))));
    var b = new PlacedFeature(null,List.of(new HeightRangePlacement(height(json))));
    var s = settings(a);
    out.println(label+" "+s.hasFeature(a)+" "+s.hasFeature(b));
  }
  public static void main(String[] args) throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    compare(out,"uniform","{\"type\":\"minecraft:uniform\",\"min_inclusive\":{\"absolute\":0},\"max_inclusive\":{\"absolute\":8}}");
    compare(out,"trapezoid","{\"type\":\"minecraft:trapezoid\",\"min_inclusive\":{\"absolute\":0},\"max_inclusive\":{\"absolute\":8}}");
    var a=new PlacedFeature(null,List.of()); var b=new PlacedFeature(null,List.of());
    var s=settings(a); out.println("none "+s.hasFeature(a)+" "+s.hasFeature(b));
  }
}
