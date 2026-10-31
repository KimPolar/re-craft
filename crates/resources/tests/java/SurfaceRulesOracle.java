import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.DoubleSupplier;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.levelgen.WorldGenerationContext;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.material.MaterialRuleContext;
import net.minecraft.world.level.levelgen.material.rule.MaterialRule;
import net.minecraft.world.level.block.Blocks;

public class SurfaceRulesOracle {
    static void set(Object o,String name,Object value) throws Exception {var f=o.getClass().getDeclaredField(name);f.setAccessible(true);f.set(o,value);}
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        LevelAccessor level=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(obj,method,a)->switch(method.getName()){
            case "getMinY" -> -64; case "getHeight" -> 384; case "getSeaLevel" -> 63;
            default -> throw new UnsupportedOperationException(method.getName());
        });
        var worldCtor=WorldGenerationContext.class.getDeclaredConstructor(LevelAccessor.class);worldCtor.setAccessible(true);
        var world=worldCtor.newInstance(level);
        var ctor=MaterialRuleContext.class.getDeclaredConstructors()[0];ctor.setAccessible(true);
        var ctx=(MaterialRuleContext)ctor.newInstance(null,null,new DensityVolume(16,128,16,0,-64,0),null,null,world,Set.of());
        var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");unsafeField.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)unsafeField.get(null);
        var randomState=(RandomState)unsafe.allocateInstance(RandomState.class);
        var name=Identifier.withDefaultNamespace("bedrock_floor");
        var factory=new XoroshiroRandomSource(12345).forkPositional().fromHashOf(name).forkPositional();
        var factories=new ConcurrentHashMap<>();factories.put(name,factory);set(randomState,"positionalRandoms",factories);set(ctx,"randomState",randomState);
        int[] current={0};
        var noiseKey=ResourceKey.create(Registries.NOISE,Identifier.withDefaultNamespace("patch"));
        for(String field:new String[]{"noiseSamplers2d","noiseSamplers3d"}) {
            var f=ctx.getClass().getDeclaredField(field);f.setAccessible(true);
            ((java.util.Map)f.get(ctx)).put(noiseKey,(DoubleSupplier)()->(current[0]%5-2)*0.5);
        }
        var rules=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        for(int i=0;i<rules.size();i++) {
            var rule=MaterialRule.DIRECT_CODEC.parse(JsonOps.INSTANCE,rules.get(i)).getOrThrow().compile(ctx);
            out.printf("S %d%n",i);
            for(int n=0;n<512;n++) {
                int y=(n*37)%211-100,above=(n*7)%11,below=(n*13)%11,depth=(n*5)%9-2;
                current[0]=n;
                int water=n%5==0?Integer.MIN_VALUE:(n*17)%181-90;
                int minSurface=(n*19)%201-100, gx=(n*3)%13-6,gz=(n*11)%13-6;
                double secondary=((n*7)%25-12)/8.0;
                set(ctx,"lastUpdateXZ",(long)n+1000);set(ctx,"lastUpdateY",(long)n+1000);
                set(ctx,"lastMinSurfaceLevelUpdate",(long)n+1000);set(ctx,"lastSurfaceDepth2Update",(long)n+1000);
                set(ctx,"blockY",y);set(ctx,"stoneDepthAbove",above);set(ctx,"stoneDepthBelow",below);
                set(ctx,"blockX",n);set(ctx,"blockZ",-n);
                set(ctx,"surfaceDepth",depth);set(ctx,"surfaceSecondary",secondary);set(ctx,"waterHeight",water);
                set(ctx,"minSurfaceLevel",minSurface);set(ctx,"surfaceGradientX",gx);set(ctx,"surfaceGradientZ",gz);
                var result=rule.tryApply(n,y,-n);
                int code=result==null?0:result.is(Blocks.STONE)?1:result.is(Blocks.DIRT)?2:result.is(Blocks.GRASS_BLOCK)?3:-1;
                if(code<0) throw new AssertionError(result);
                out.printf("P %d %d%n",n,code);
            }
        }
    }
}
