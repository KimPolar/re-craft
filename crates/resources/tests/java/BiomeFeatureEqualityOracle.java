import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.Constructor;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderSet;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.resources.RegistryOps;
import net.minecraft.world.level.biome.BiomeGenerationSettings;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.placement.BlockPredicateFilter;
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
  static void comparePredicate(java.io.PrintStream out, RegistryOps<com.google.gson.JsonElement> ops, String label, String json) throws Exception {
    var a=BlockPredicate.CODEC.parse(ops,JsonParser.parseString(json)).getOrThrow();
    var b=BlockPredicate.CODEC.parse(ops,JsonParser.parseString(json)).getOrThrow();
    var first=new PlacedFeature(null,List.of(new BlockPredicateFilter(a)));
    var second=new PlacedFeature(null,List.of(new BlockPredicateFilter(b)));
    var s=settings(first);out.println(label+" "+s.hasFeature(first)+" "+s.hasFeature(second));
  }
  public static void main(String[] args) throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    compare(out,"uniform","{\"type\":\"minecraft:uniform\",\"min_inclusive\":{\"absolute\":0},\"max_inclusive\":{\"absolute\":8}}");
    compare(out,"trapezoid","{\"type\":\"minecraft:trapezoid\",\"min_inclusive\":{\"absolute\":0},\"max_inclusive\":{\"absolute\":8}}");
    var a=new PlacedFeature(null,List.of()); var b=new PlacedFeature(null,List.of());
    var s=settings(a); out.println("none "+s.hasFeature(a)+" "+s.hasFeature(b));
    var lookup=VanillaRegistries.createWorldLookup();
    var features=lookup.lookupOrThrow(Registries.FEATURE);
    var flower=features.getOrThrow(ResourceKey.create(Registries.FEATURE,Identifier.parse("minecraft:flower_pale_garden")));
    var other=features.getOrThrow(ResourceKey.create(Registries.FEATURE,Identifier.parse("minecraft:ore_dirt")));
    var flowerAgain=features.getOrThrow(ResourceKey.create(Registries.FEATURE,Identifier.parse("minecraft:flower_pale_garden")));
    var registered=new PlacedFeature(flower,List.of());
    var holderSettings=settings(registered);
    out.println("holders "+holderSettings.hasFeature(new PlacedFeature(flowerAgain,List.of()))+" "+holderSettings.hasFeature(new PlacedFeature(other,List.of()))+" "+holderSettings.hasFeature(new PlacedFeature(Holder.direct(flower.value()),List.of())));
    var ops=RegistryOps.create(JsonOps.INSTANCE,lookup);
    comparePredicate(out,ops,"blocks","{\"type\":\"minecraft:matching_blocks\",\"blocks\":\"minecraft:stone\"}");
    comparePredicate(out,ops,"fluids","{\"type\":\"minecraft:matching_fluids\",\"fluids\":\"minecraft:water\"}");
    comparePredicate(out,ops,"tag","{\"type\":\"minecraft:matching_block_tag\",\"tag\":\"minecraft:base_stone_overworld\"}");
    comparePredicate(out,ops,"solid","{\"type\":\"minecraft:solid\"}");
    comparePredicate(out,ops,"not","{\"type\":\"minecraft:not\",\"predicate\":{\"type\":\"minecraft:true\"}}");
    comparePredicate(out,ops,"any","{\"type\":\"minecraft:any_of\",\"predicates\":[{\"type\":\"minecraft:true\"}]}");
    comparePredicate(out,ops,"all","{\"type\":\"minecraft:all_of\",\"predicates\":[{\"type\":\"minecraft:true\"}]}");
    comparePredicate(out,ops,"true","{\"type\":\"minecraft:true\"}");
  }
}
