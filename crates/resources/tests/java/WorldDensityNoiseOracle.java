import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.IdentityHashMap;
import java.util.HashSet;
import com.google.gson.JsonElement;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunctions;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunctionCompiler;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.synth.Noise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public class WorldDensityNoiseOracle {
    // Codec-decoded noise values may compare equal even when their registry names
    // (and therefore their seeds) differ. Match each parsed holder to the name
    // annotated in its original JSON, using object identity rather than equality.
    static void bindNoiseNames(Object node, JsonElement json, IdentityHashMap<NormalNoise,String> names) throws Exception {
        if (node == null || json == null || !json.isJsonObject() || !node.getClass().isRecord()) return;
        var object = json.getAsJsonObject();
        for (var field : node.getClass().getRecordComponents()) {
            Object child = field.getAccessor().invoke(node);
            String key = field.getName().replaceAll("([A-Z])", "_$1").toLowerCase(java.util.Locale.ROOT);
            if (key.equals("noise") && child instanceof Holder<?> holder && object.has("_oracle_noise_id")) {
                String name = object.get("_oracle_noise_id").getAsString();
                String previous = names.put((NormalNoise)holder.value(), name);
                if (previous != null && !previous.equals(name)) throw new IllegalStateException("noise identity was shared by different names");
            } else if (child instanceof DensityFunction) {
                bindNoiseNames(child, object.get(key), names);
            } else if (child instanceof java.util.List<?> list && object.has(key) && object.get(key).isJsonArray()) {
                var array = object.getAsJsonArray(key);
                for (int j=0;j<list.size() && j<array.size();j++) bindNoiseNames(list.get(j), array.get(j), names);
            }
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var fixtures = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        long[] seeds = {0, -1, 2345, Long.MIN_VALUE, Long.MAX_VALUE};
        int[][] points = {{0,0,0},{1,1,1},{-1,-1,-1},{31,63,-80},{-17,-64,49},
            {12345,-20,-9832},{30000000,100,-30000000},{Integer.MIN_VALUE,12,7},
            {Integer.MAX_VALUE,-31,-17},{0,319,0},{0,320,0},{7,13,19},
            {1023,0,0},{1024,0,0},{1025,0,0},{-1023,0,0},{-1024,0,0},{-1025,0,0},
            {1500,17,1500},{-1500,-17,-1500},{4096,20,-4096},{-4096,20,4096},
            {8191,7,-7},{-8191,7,7}};
        for (int i=0;i<fixtures.size();i++) {
            var fixture=fixtures.get(i).getAsJsonObject();
            String id=fixture.get("id").getAsString();
            var noiseNames = new HashMap<NormalNoise,String>();
            var ambiguousNames = new HashSet<NormalNoise>();
            if (fixture.has("noises")) {
                for (var entry:fixture.getAsJsonObject("noises").entrySet()) {
                    var definition=NormalNoise.DIRECT_CODEC.parse(JsonOps.INSTANCE,entry.getValue()).getOrThrow();
                    String previous=noiseNames.put(definition,entry.getKey());
                    if (previous != null && !previous.equals(entry.getKey())) ambiguousNames.add(definition);
                }
            }
            var function=DensityFunctions.DIRECT_CODEC.parse(JsonOps.INSTANCE,fixture.get("java_expression")).getOrThrow();
            var identityNames = new IdentityHashMap<NormalNoise,String>();
            bindNoiseNames(function, fixture.get("java_expression"), identityNames);
            for (int mode=0;mode<2;mode++) for (long seed:seeds) {
                if (fixture.has("router_mode") && fixture.get("router_mode").getAsInt() != mode) continue;
                boolean legacy = mode == 0;
                RandomSource root=mode==0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
                var factory=root.forkPositional();
                var ctx=new DensityFunction.CompileContext() {
                    public Noise createNoiseSampler(Holder<NormalNoise> noise) {
                        if (ambiguousNames.contains(noise.value()) && !identityNames.containsKey(noise.value())) throw new IllegalStateException("ambiguous noise identity");
                        String name=identityNames.getOrDefault(noise.value(),noiseNames.getOrDefault(noise.value(),id));
                        if (name.equals("minecraft:nether/temperature")) return noise.value().createForLegacyNetherBiome(new LegacyRandomSource(seed));
                        if (name.equals("minecraft:nether/vegetation")) return noise.value().createForLegacyNetherBiome(new LegacyRandomSource(seed + 1));
                        return noise.value().create(factory.fromHashOf(Identifier.parse(name)));
                    }
                    public RandomSource createRandom(Identifier name) {
                        if (legacy && name.equals(Identifier.parse("minecraft:terrain"))) return new LegacyRandomSource(seed);
                        return factory.fromHashOf(name);
                    }
                    public RandomSource createEndIslandRandom() { return new LegacyRandomSource(seed); }
                };
                var sampler=new DensityFunctionCompiler(ctx).getSampler(function);
                var range=function.range();
                System.out.printf("S %d %d %d %d %d%n",i,mode,seed,Float.floatToRawIntBits(range.min()),Float.floatToRawIntBits(range.max()));
                for (var p:points) {
                    System.out.printf("P %d %d %d %d%n",p[0],p[1],p[2],Float.floatToRawIntBits(sampler.sampleValue(SamplerContext.EMPTY_UNCACHED,p[0],p[1],p[2])));
                }
                if (fixture.has("end_fixture")) for (int j=0;j<80;j++) {
                    int x=(j*7919)%90000-45000, z=(j*3571)%90000-45000, y=j-40;
                    System.out.printf("Q %d %d %d %d%n",x,y,z,Float.floatToRawIntBits(sampler.sampleValue(SamplerContext.EMPTY_UNCACHED,x,y,z)));
                }
            }
        }
    }
}
