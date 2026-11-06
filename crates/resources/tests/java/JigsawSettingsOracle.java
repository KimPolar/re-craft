import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.heightproviders.HeightProvider;
import net.minecraft.world.level.levelgen.structure.pools.DimensionPadding;
import net.minecraft.world.level.levelgen.structure.structures.JigsawStructure;
import net.minecraft.world.level.levelgen.structure.templatesystem.LiquidSettings;

public class JigsawSettingsOracle {
    static Object field(Object object,String name) throws Exception {
        Field f=object.getClass().getDeclaredField(name); f.setAccessible(true); return f.get(object);
    }
    static JsonObject definition(String name) throws Exception {
        try(var stream=JigsawSettingsOracle.class.getClassLoader().getResourceAsStream("data/minecraft/worldgen/structure/"+name+".json")) {
            return JsonParser.parseString(new String(stream.readAllBytes(),StandardCharsets.UTF_8)).getAsJsonObject();
        }
    }
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var ops=VanillaRegistries.createWorldLookup().createSerializationContext(JsonOps.INSTANCE);
        var inputs=new ArrayList<JsonObject>();
        for(String name:new String[]{"ancient_city","trial_chambers","village_plains","abandoned_camp_forest"}) inputs.add(definition(name));
        var synthetic=definition("ancient_city");
        synthetic.add("max_distance_from_center",JsonParser.parseString("{\"horizontal\":100}"));
        synthetic.add("dimension_padding",JsonParser.parseString("{\"bottom\":-1,\"top\":3}"));
        synthetic.add("start_height",JsonParser.parseString("{\"type\":\"minecraft:biased_to_bottom\",\"min_inclusive\":{\"absolute\":-30},\"max_inclusive\":{\"relative_to_sea_level\":0}}"));
        inputs.add(synthetic);
        var invalid=definition("ancient_city"); invalid.addProperty("max_distance_from_center",120); inputs.add(invalid);
        Field uf=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); uf.setAccessible(true); var unsafe=(sun.misc.Unsafe)uf.get(null);
        var dimensions=(WorldGenerationContext)unsafe.allocateInstance(WorldGenerationContext.class);
        for(var e:Map.of("minY",-64,"height",384,"seaLevel",63).entrySet()) { var f=WorldGenerationContext.class.getDeclaredField(e.getKey()); f.setAccessible(true); f.setInt(dimensions,e.getValue()); }
        for(int id=0;id<inputs.size();id++) {
            var json=inputs.get(id); out.printf("D\t%d\t%s%n",id,json);
            var parsed=JigsawStructure.CODEC.codec().parse(ops,json).result();
            if(parsed.isEmpty()) { out.printf("C\t%d\t-\n",id); continue; }
            var structure=parsed.get(); var distance=(JigsawStructure.MaxDistance)field(structure,"maxDistanceFromCenter");
            var padding=(DimensionPadding)field(structure,"dimensionPadding");
            var liquid=(LiquidSettings)field(structure,"liquidSettings");
            var name=(Optional<?>)field(structure,"startJigsawName");
            var map=(Optional<?>)field(structure,"projectStartToHeightmap");
            out.printf("C\t%d\t%s\t%s\t%d\t%s\t%d\t%d\t%s\t%d\t%d\t%s\t%s%n",id,structure.getStartPool().unwrapKey().orElseThrow().identifier(),name.map(Object::toString).orElse("-"),field(structure,"maxDepth"),field(structure,"useExpansionHack"),distance.horizontal(),distance.vertical(),padding==DimensionPadding.ZERO?"default":"explicit",padding.bottom(),padding.top(),liquid.getSerializedName(),map.map(Object::toString).orElse("-"));
            var height=(HeightProvider)field(structure,"startHeight");
            for(long seed:new long[]{0,42,-1,Long.MIN_VALUE}) for(int[] chunk:new int[][]{{0,0},{-3,2},{123,-456}}) {
                var random=new WorldgenRandom(new LegacyRandomSource(0)); random.setLargeFeatureSeed(seed,chunk[0],chunk[1]);
                int y=height.sample(random,dimensions); var pos=new BlockPos(chunk[0]<<4,y,chunk[1]<<4);
                var aliasRandom=RandomSource.create(seed).forkPositional().at(pos);
                var aliases=new TreeMap<String,String>();
                for(var binding:structure.getPoolAliases()) binding.forEachResolved(aliasRandom,(a,b)->aliases.put(a.identifier().toString(),b.identifier().toString()));
                var encoded=new StringBuilder(); aliases.forEach((a,b)->encoded.append('|').append(a).append('=').append(b));
                out.printf("S\t%d\t%d\t%d\t%d\t%d\t%d%s%n",id,seed,chunk[0],chunk[1],y,random.nextLong(),encoded);
            }
        }
    }
}
