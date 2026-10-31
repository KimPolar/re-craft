import java.lang.reflect.*;
import java.util.Random;
import it.unimi.dsi.fastutil.longs.Long2IntMap;
import net.minecraft.SharedConstants;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.*;

public class AquiferFluidOracle {
    static int base, sea, mode;
    static float exclusion, flooded, spread, lava;
    static BlockState water() { return Blocks.WATER.defaultBlockState(); }
    static BlockState lava() { return Blocks.LAVA.defaultBlockState(); }
    static Aquifer.FluidStatus global(int x, int y, int z) {
        if (mode == 1 && y < Math.min(sea, -54)) return new Aquifer.FluidStatus(-54, lava());
        if (mode == 2) {
            int cell = Math.floorDiv(x,16) ^ Math.floorDiv(z,16);
            return new Aquifer.FluidStatus(sea + Math.floorMod(cell,17)-8, (cell & 3) == 0 ? lava() : water());
        }
        return new Aquifer.FluidStatus(sea,water());
    }
    static float sample(int id,int x,int y,int z) {
        return switch (id) {
            case 1 -> exclusion;
            case 2 -> flooded;
            case 3 -> spread + ((x*7+y*11+z*13 & 7)-3)*0.03125f;
            case 4 -> lava + ((x*3+y*5+z*7 & 7)-3)*0.03125f;
            case 5 -> base + ((x/4*31+z/4*17 & 31)-16)*0.25f;
            default -> 0;
        };
    }
    static class Sampler implements DensitySampler {
        final int id;
        Sampler(int id) { this.id=id; }
        public float sampleValue(SamplerContext c,int x,int y,int z) { return sample(id,x,y,z); }
        public void sampleVolume(SamplerContext c,DensityBuffer b,DensityVolume v) { DensitySampler.sampleVolumeNaive(c,b,v,this); }
    }
    public static void main(String[] args) throws Exception {
        var out=System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        DensityFunction barrier=DensityFunctions.constant(101), flood=DensityFunctions.constant(102),
            shift=DensityFunctions.constant(103), lavaNoise=DensityFunctions.constant(104),
            exclude=DensityFunctions.constant(105), surface=DensityFunctions.constant(106);
        var config=new Aquifer.Config(barrier,flood,shift,lavaNoise,exclude,surface);
        DensitySamplerSet samplers=f -> new DensitySampler.Bound(new Sampler(f==exclude?1:f==flood?2:f==shift?3:f==lavaNoise?4:f==surface?5:0),SamplerContext.EMPTY_UNCACHED);
        var factory=new XoroshiroRandomSource(0).forkPositional().fromHashOf(Identifier.withDefaultNamespace("aquifer")).forkPositional();
        var aquifer=config.create(samplers,factory,new DensityVolume(16,128,16,0,-64,0),AquiferFluidOracle::global);
        var cls=aquifer.getClass();
        var method=cls.getDeclaredMethod("computeFluid",int.class,int.class,int.class); method.setAccessible(true);
        var cache=cls.getDeclaredField("surfaceLevelCache"); cache.setAccessible(true);
        var statusCache=cls.getDeclaredField("aquiferCache"); statusCache.setAccessible(true);
        var skip=cls.getDeclaredField("skipSamplingAboveY"); skip.setAccessible(true);
        var random=new Random(91235);
        float[] floods={-1.1f,-0.8f,-0.3f,0,0.4f,0.8f,1.1f,Float.NaN};
        float[] spreads={-1,-0.3f,0,0.3f,1};
        float[] lavas={-0.7f,-0.3f,0,0.3f,0.7f};
        for(int n=0;n<8000;n++) {
            int x=random.nextInt(2001)-1000,y=random.nextInt(311)-120,z=random.nextInt(2001)-1000;
            base=random.nextInt(161)-50; sea=random.nextInt(140)-70; mode=random.nextInt(3);
            exclusion=random.nextInt(3)-1; flooded=floods[random.nextInt(floods.length)];
            spread=spreads[random.nextInt(spreads.length)]; lava=lavas[random.nextInt(lavas.length)];
            ((Long2IntMap)cache.get(aquifer)).clear();
            var result=(Aquifer.FluidStatus)method.invoke(aquifer,x,y,z);
            out.printf("F %d %d %d %d %d %d %d %d %d %d %d %d%n",x,y,z,base,sea,mode,
                Float.floatToRawIntBits(exclusion),Float.floatToRawIntBits(flooded),Float.floatToRawIntBits(spread),Float.floatToRawIntBits(lava),
                result.fluidLevel(),result.fluidType().is(Blocks.LAVA)?2:result.fluidType().is(Blocks.WATER)?1:-1);
        }
        double[] densities={-100,-4,-1,-0.5,-0.01,-0.0,1,Double.NaN};
        for(int batch=0;batch<40;batch++) {
            base=random.nextInt(161)-50; sea=random.nextInt(140)-70; mode=random.nextInt(3);
            exclusion=random.nextInt(3)-1; flooded=floods[random.nextInt(floods.length)];
            spread=spreads[random.nextInt(spreads.length)]; lava=lavas[random.nextInt(lavas.length)];
            ((Long2IntMap)cache.get(aquifer)).clear();
            java.util.Arrays.fill((Object[])statusCache.get(aquifer),null);
            out.printf("B %d %d %d %d %d %d %d %d%n",base,sea,mode,Float.floatToRawIntBits(exclusion),
                Float.floatToRawIntBits(flooded),Float.floatToRawIntBits(spread),Float.floatToRawIntBits(lava),skip.getInt(aquifer));
            for(int n=0;n<50;n++) {
                int x=random.nextInt(16), y=random.nextInt(128)-64, z=random.nextInt(16);
                double density=densities[random.nextInt(densities.length)];
                BlockState state=aquifer.computeSubstance(x,y,z,density);
                int result=state==null?-1:state.is(Blocks.AIR)?0:state.is(Blocks.WATER)?1:state.is(Blocks.LAVA)?2:-2;
                out.printf("D %d %d %d %d %d %d%n",x,y,z,Double.doubleToRawLongBits(density),result,aquifer.shouldScheduleFluidUpdate()?1:0);
            }
        }
    }
}
