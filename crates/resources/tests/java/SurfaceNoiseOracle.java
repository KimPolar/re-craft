import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.DoubleSupplier;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.material.MaterialRuleContext;
import net.minecraft.world.level.levelgen.material.MaterialSystem;
import net.minecraft.world.level.levelgen.material.condition.NoiseThresholdCondition;
import net.minecraft.world.level.levelgen.material.condition.ConditionEvaluator;
import net.minecraft.world.level.levelgen.synth.Noise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public class SurfaceNoiseOracle {
    static void set(Object o,String name,Object value)throws Exception {var f=o.getClass().getDeclaredField(name);f.setAccessible(true);f.set(o,value);}
    public static void main(String[] args)throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var defs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
        var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");unsafeField.setAccessible(true);var unsafe=(sun.misc.Unsafe)unsafeField.get(null);
        var contextCtor=MaterialRuleContext.class.getDeclaredConstructors()[0];contextCtor.setAccessible(true);
        var depthMethod=MaterialSystem.class.getDeclaredMethod("getSurfaceDepth",int.class,int.class);depthMethod.setAccessible(true);
        var secondaryMethod=MaterialSystem.class.getDeclaredMethod("getSurfaceSecondary",int.class,int.class);secondaryMethod.setAccessible(true);
        int[][] points={{0,0,0},{1,1,1},{1,63,1},{-1,-1,-1},{31,63,-80},{-17,-64,49},
            {12345,-20,-9832},{30000000,100,-30000000},{Integer.MIN_VALUE,12,7},
            {Integer.MAX_VALUE,-31,-17},{0,319,0},{0,320,0},{7,13,19},{-1024,0,1024}};
        for(int mode=0;mode<2;mode++) for(long seed:new long[]{0,-1,2345,Long.MIN_VALUE,Long.MAX_VALUE}) {
            RandomSource initial=mode==0?new LegacyRandomSource(seed):new XoroshiroRandomSource(seed);
            var root=initial.forkPositional();
            var noiseMap=new ConcurrentHashMap<ResourceKey<NormalNoise>,Noise>();
            for(var entry:defs.entrySet()) {
                var id=Identifier.parse(entry.getKey());
                var key=ResourceKey.create(Registries.NOISE,id);
                var params=NormalNoise.DIRECT_CODEC.parse(JsonOps.INSTANCE,entry.getValue()).getOrThrow();
                noiseMap.put(key,params.create(root.fromHashOf(id)));
            }
            var rs=(RandomState)unsafe.allocateInstance(RandomState.class);set(rs,"noiseInstances",noiseMap);
            var context=(MaterialRuleContext)contextCtor.newInstance(null,rs,new DensityVolume(1,1,1,0,0,0),null,null,null,Set.of());
            var system=(MaterialSystem)unsafe.allocateInstance(MaterialSystem.class);
            set(system,"surfaceNoise",noiseMap.get(ResourceKey.create(Registries.NOISE,Identifier.withDefaultNamespace("surface"))));
            set(system,"surfaceSecondaryNoise",noiseMap.get(ResourceKey.create(Registries.NOISE,Identifier.withDefaultNamespace("surface_secondary"))));
            set(system,"noiseRandom",root);
            var samplers=new LinkedHashMap<String,DoubleSupplier[]>();
            var tests=new LinkedHashMap<String,ConditionEvaluator[]>();
            for(var entry:defs.entrySet()) {var key=ResourceKey.create(Registries.NOISE,Identifier.parse(entry.getKey()));
                samplers.put(entry.getKey(),new DoubleSupplier[]{context.getNoiseSampler(key,false),context.getNoiseSampler(key,true)});
                tests.put(entry.getKey(),new ConditionEvaluator[]{new NoiseThresholdCondition(key,-0.125,0.125,false).compile(context),new NoiseThresholdCondition(key,-0.125,0.125,true).compile(context)});}
            out.printf("S %d %d%n",mode,seed);
            int priorX=0,priorZ=0;long xz=100;
            for(int n=0;n<points.length;n++) {int x=points[n][0],y=points[n][1],z=points[n][2];
                if(n==0||x!=priorX||z!=priorZ)xz++;
                priorX=x;priorZ=z;set(context,"lastUpdateXZ",xz);set(context,"lastUpdateY",(long)n+1000);
                set(context,"blockX",x);set(context,"blockY",y);set(context,"blockZ",z);
                for(var entry:samplers.entrySet()) out.printf("P %s %d %d %d %d %d %d %d%n",entry.getKey(),x,y,z,
                    Double.doubleToRawLongBits(entry.getValue()[0].getAsDouble()),Double.doubleToRawLongBits(entry.getValue()[1].getAsDouble()),
                    tests.get(entry.getKey())[0].test()?1:0,tests.get(entry.getKey())[1].test()?1:0);
                out.printf("D %d %d %d %d%n",x,z,(int)depthMethod.invoke(system,x,z),Double.doubleToRawLongBits((double)secondaryMethod.invoke(system,x,z)));
            }
        }
    }
}
