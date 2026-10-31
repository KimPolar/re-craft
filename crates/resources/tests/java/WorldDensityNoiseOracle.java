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
import net.minecraft.world.level.levelgen.densityfunction.DensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.densityfunction.DensitySampler;
import net.minecraft.world.level.levelgen.densityfunction.DensitySamplerSet;
import net.minecraft.world.level.levelgen.synth.Noise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.material.MaterialRuleContext;
import net.minecraft.world.level.levelgen.material.rule.OreVeinRule;

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
                Aquifer.Config localAquifer = null;
                if (fixture.has("java_aquifers")) {
                    var aq=fixture.getAsJsonObject("java_aquifers");
                    String[] fields={"barrier","fluid_level_floodedness","fluid_level_spread","lava","exclusion","surface_level"};
                    var functions=new DensityFunction[fields.length];
                    for(int j=0;j<fields.length;j++) {
                        functions[j]=DensityFunctions.DIRECT_CODEC.parse(JsonOps.INSTANCE,aq.get(fields[j])).getOrThrow();
                        bindNoiseNames(functions[j],aq.get(fields[j]),identityNames);
                    }
                    localAquifer=new Aquifer.Config(functions[0],functions[1],functions[2],functions[3],functions[4],functions[5]);
                }
            DensityFunction[] oreFunctions=null;
            if (fixture.has("java_ore_functions")) {
                var definitions=fixture.getAsJsonArray("java_ore_functions");
                oreFunctions=new DensityFunction[3];
                for(int j=0;j<3;j++) {
                    oreFunctions[j]=DensityFunctions.DIRECT_CODEC.parse(JsonOps.INSTANCE,definitions.get(j)).getOrThrow();
                    bindNoiseNames(oreFunctions[j],definitions.get(j),identityNames);
                }
            }
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
                var compiler=new DensityFunctionCompiler(ctx);
                var sampler=compiler.getSampler(function);
                var range=function.range();
                System.out.printf("S %d %d %d %d %d%n",i,mode,seed,Float.floatToRawIntBits(range.min()),Float.floatToRawIntBits(range.max()));
                for (var p:points) {
                    System.out.printf("P %d %d %d %d%n",p[0],p[1],p[2],Float.floatToRawIntBits(sampler.sampleValue(SamplerContext.EMPTY_UNCACHED,p[0],p[1],p[2])));
                }
                if (oreFunctions!=null && (seed==0 || seed==2345)) {
                    var min=fixture.getAsJsonObject("ore_min_by_seed").getAsJsonArray(Long.toString(seed));var size=fixture.getAsJsonArray("ore_size");
                    int minX=min.get(0).getAsInt(),minY=min.get(1).getAsInt(),minZ=min.get(2).getAsInt();
                    int sx=size.get(0).getAsInt(),sy=size.get(1).getAsInt(),sz=size.get(2).getAsInt();
                    var volume=new DensityVolume(sx,sy,sz,minX,minY,minZ);
                    DensitySamplerSet set=f -> new DensitySampler.Bound(compiler.getSampler(f),SamplerContext.EMPTY_UNCACHED);
                    var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");unsafeField.setAccessible(true);
                    var unsafe=(sun.misc.Unsafe)unsafeField.get(null);
                    var rs=(RandomState)unsafe.allocateInstance(RandomState.class);
                    var randoms=new java.util.concurrent.ConcurrentHashMap<>();
                    var oreName=Identifier.withDefaultNamespace("ore");randoms.put(oreName,factory.fromHashOf(oreName).forkPositional());
                    var randomField=RandomState.class.getDeclaredField("positionalRandoms");randomField.setAccessible(true);randomField.set(rs,randoms);
                    var ctor=MaterialRuleContext.class.getDeclaredConstructors()[0];ctor.setAccessible(true);
                    var materialContext=(MaterialRuleContext)ctor.newInstance(null,rs,volume,set,null,null,java.util.Set.of());
                    var rx=MaterialRuleContext.class.getDeclaredField("blockX");rx.setAccessible(true);
                    var ry=MaterialRuleContext.class.getDeclaredField("blockY");ry.setAccessible(true);
                    var rz=MaterialRuleContext.class.getDeclaredField("blockZ");rz.setAccessible(true);
                    var ruleJson=fixture.getAsJsonObject("rust_ore_rule");
                    var ore=BlockState.CODEC.parse(JsonOps.INSTANCE,ruleJson.get("ore_block")).getOrThrow();
                    var raw=BlockState.CODEC.parse(JsonOps.INSTANCE,ruleJson.get("raw_ore_block")).getOrThrow();
                    var filler=BlockState.CODEC.parse(JsonOps.INSTANCE,ruleJson.get("filler_block")).getOrThrow();
                    var oreRule=new OreVeinRule(ore,raw,filler,ruleJson.get("raw_ore_chance").getAsFloat(),oreFunctions[0],oreFunctions[1],oreFunctions[2]).compile(materialContext);
                    for(int z=0;z<sz;z++) for(int x=0;x<sx;x++) for(int y=0;y<sy;y++) {
                        int bx=minX+x,by=minY+y,bz=minZ+z;rx.setInt(materialContext,bx);ry.setInt(materialContext,by);rz.setInt(materialContext,bz);
                        var block=oreRule.tryApply(bx,by,bz);
                        int code=block==null?0:block==ore?1:block==raw?2:block==filler?3:-1;
                        if(code<0)throw new AssertionError(block);
                        System.out.printf("O %d%n",code);
                    }
                }
                if (fixture.has("volume_min")) {
                    var min=fixture.getAsJsonArray("volume_min");
                    var size=fixture.getAsJsonArray("volume_size");
                    var volume=new DensityVolume(size.get(0).getAsInt(),size.get(1).getAsInt(),size.get(2).getAsInt(),min.get(0).getAsInt(),min.get(1).getAsInt(),min.get(2).getAsInt());
                    var buffer=DensityBuffer.createUnpooled(volume.size());
                    sampler.sampleVolume(SamplerContext.EMPTY_UNCACHED,buffer,volume);
                    for(int z=0;z<volume.sizeZ();z++) for(int x=0;x<volume.sizeX();x++) for(int y=0;y<volume.sizeY();y++)
                        System.out.printf("V %d %d %d %d%n",x,y,z,Float.floatToRawIntBits(buffer.get(volume.indexUnchecked(x,y,z))));
                }
                if (fixture.has("chunk_height") && seed == 0) {
                    int minY=fixture.get("chunk_min_y").getAsInt(), height=fixture.get("chunk_height").getAsInt();
                    var volume=new DensityVolume(16,height,16,90*16,minY,-60*16);
                    var buffer=DensityBuffer.createUnpooled(volume.size());
                    sampler.sampleVolume(SamplerContext.EMPTY_UNCACHED,buffer,volume);
                    for (int j=0;j<buffer.size();j++) System.out.printf("C %d%n",Float.floatToRawIntBits(buffer.get(j)));
                    if (fixture.has("simple_noise") || localAquifer != null) {
                        boolean nether=fixture.has("simple_noise") && fixture.get("simple_noise").getAsString().equals("nether");
                        var fluid=localAquifer!=null?Blocks.WATER.defaultBlockState():nether?Blocks.LAVA.defaultBlockState():Blocks.AIR.defaultBlockState();
                        int sea=localAquifer!=null?63:nether?32:0;
                        var setting=new NoiseGeneratorSettings(null,Blocks.STONE.defaultBlockState(),fluid,null,null,java.util.List.of(),sea,false,java.util.Optional.empty(),true,null);
                        var method=NoiseBasedChunkGenerator.class.getDeclaredMethod("createFluidPicker", NoiseGeneratorSettings.class);
                        method.setAccessible(true);
                        var picker=(Aquifer.FluidPicker)method.invoke(null,setting);
                        DensitySamplerSet samplerSet=f -> new DensitySampler.Bound(compiler.getSampler(f),SamplerContext.EMPTY_UNCACHED);
                        var aquifer=localAquifer==null?Aquifer.createDisabled(picker):localAquifer.create(samplerSet,factory.fromHashOf(Identifier.withDefaultNamespace("aquifer")).forkPositional(),volume,picker);
                        for (int j=0;j<buffer.size();j++) {
                            int y=minY+j%height;
                            int x=90*16+(j/height)%16,z=-60*16+(j/height)/16;
                            var state=aquifer.computeSubstance(x,y,z,buffer.get(j));
                            int code=state==null?0:state.is(Blocks.WATER)?1:state.is(Blocks.LAVA)?2:state.is(Blocks.AIR)?3:-1;
                            System.out.printf("M %d %d%n",code,aquifer.shouldScheduleFluidUpdate()?1:0);
                        }
                    }
                }
                if (fixture.has("end_fixture")) for (int j=0;j<80;j++) {
                    int x=(j*7919)%90000-45000, z=(j*3571)%90000-45000, y=j-40;
                    System.out.printf("Q %d %d %d %d%n",x,y,z,Float.floatToRawIntBits(sampler.sampleValue(SamplerContext.EMPTY_UNCACHED,x,y,z)));
                }
            }
        }
        var pickerMethod=NoiseBasedChunkGenerator.class.getDeclaredMethod("createFluidPicker", NoiseGeneratorSettings.class);
        pickerMethod.setAccessible(true);
        int[] levels={-100,-54,-53,0,32,63,320};
        int[] ys={-110,-101,-100,-99,-55,-54,-53,-1,0,31,32,62,63,319,320,321};
        float[] densities={Float.NEGATIVE_INFINITY,-1.0f,-0.0f,0.0f,1.0f,Float.POSITIVE_INFINITY,Float.NaN};
        for (int sea:levels) {
            var settings=new NoiseGeneratorSettings(null,Blocks.STONE.defaultBlockState(),Blocks.WATER.defaultBlockState(),null,null,java.util.List.of(),sea,false,java.util.Optional.empty(),true,null);
            var picker=(Aquifer.FluidPicker)pickerMethod.invoke(null,settings);
            var aquifer=Aquifer.createDisabled(picker);
            for (int y:ys) for (float density:densities) {
                var state=aquifer.computeSubstance(0,y,0,density);
                int code=state==null?0:state.is(Blocks.WATER)?1:state.is(Blocks.LAVA)?2:state.is(Blocks.AIR)?3:-1;
                System.out.printf("F %d %d %d %d%n",sea,y,Float.floatToRawIntBits(density),code);
            }
        }
    }
}
