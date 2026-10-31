import java.lang.reflect.*;
import java.util.Random;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.*;
import org.apache.commons.lang3.mutable.MutableDouble;

// Invoke the actual 26.3 pressure and fluid-selection code with synthetic statuses.
public class AquiferPressureOracle {
    static BlockState state(int n) {
        return switch(n) {
            case 0 -> Blocks.AIR.defaultBlockState();
            case 1 -> Blocks.WATER.defaultBlockState();
            case 2 -> Blocks.LAVA.defaultBlockState();
            default -> Blocks.STONE.defaultBlockState();
        };
    }
    static int code(BlockState s) {
        if (s == null) return -1;
        if (s.is(Blocks.AIR)) return 0;
        if (s.is(Blocks.WATER)) return 1;
        if (s.is(Blocks.LAVA)) return 2;
        if (s.is(Blocks.STONE)) return 3;
        throw new AssertionError(s);
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var zero = DensityFunctions.zero();
        var config = new Aquifer.Config(DensityFunctions.constant(0.25f), zero, zero, zero, zero, DensityFunctions.constant(256));
        var compiler = new DensityFunctionCompiler(null);
        DensitySamplerSet samplers = f -> new DensitySampler.Bound(compiler.getSampler(f), SamplerContext.EMPTY_UNCACHED);
        var klass = Class.forName("net.minecraft.world.level.levelgen.Aquifer$NoiseBasedAquifer");
        var index = klass.getDeclaredMethod("getIndex", int.class, int.class, int.class); index.setAccessible(true);
        var locationsField = klass.getDeclaredField("aquiferLocationCache"); locationsField.setAccessible(true);
        var fluidsField = klass.getDeclaredField("aquiferCache"); fluidsField.setAccessible(true);
        var skipField = klass.getDeclaredField("skipSamplingAboveY"); skipField.setAccessible(true);
        var pressure = klass.getDeclaredMethod("calculatePressure", int.class, int.class, int.class, MutableDouble.class, Aquifer.FluidStatus.class, Aquifer.FluidStatus.class); pressure.setAccessible(true);
        RandomSource random = new XoroshiroRandomSource(1234);
        var factory = random.forkPositional().fromHashOf(Identifier.withDefaultNamespace("aquifer")).forkPositional();
        int[] here = {0}; int[] mode = {0};
        Aquifer.FluidPicker picker = (x,y,z) -> mode[0] == 1 || (mode[0] == 2 && y < here[0])
            ? new Aquifer.FluidStatus(Integer.MAX_VALUE, state(2)) : new Aquifer.FluidStatus(63, state(1));
        var aquifer = config.create(samplers, factory, new DensityVolume(16,128,16,0,-64,0), picker);
        for (int y : new int[]{-32, 299, 319}) for (int m : new int[]{0, 1}) for (double density : new double[]{1, 0, -1, Double.NaN}) {
            // Skip positive density or cases known to return before local sampling.
            if (!(density > 0) && y <= skipField.getInt(aquifer) && m != 1) continue;
            here[0] = y; mode[0] = m;
            int actual = code(aquifer.computeSubstance(0,y,0,density));
            out.printf("E %d %d %d %d %d %d%n", y, Double.doubleToRawLongBits(density), skipField.getInt(aquifer), m, actual, aquifer.shouldScheduleFluidUpdate() ? 1 : 0);
        }
        int[] levels = {Integer.MIN_VALUE, -100, -54, -1, 0, 1, 4, 12, 63, 320, Integer.MAX_VALUE};
        int[] ys = {-64,-54,-1,0,1,12,63,319};
        double[] cached = {Double.NaN, -0.75, 0.5};
        for (int y:ys) for (int a:levels) for (int b:levels) for (int ta=0;ta<4;ta++) for (int tb=0;tb<4;tb++) for (double initial:cached) {
            var mutable = new MutableDouble(initial);
            var first = new Aquifer.FluidStatus(a,state(ta)); var second = new Aquifer.FluidStatus(b,state(tb));
            double value = (double)pressure.invoke(aquifer, 0,y,0,mutable,first,second);
            out.printf("R %d %d %d %d %d %d %d %d %d%n", y,a,ta,b,tb, Double.doubleToRawLongBits(initial), Double.doubleToRawLongBits(value), Double.doubleToRawLongBits(mutable.doubleValue()), 0);
        }
        Random samples = new Random(382901);
        double[] densities = {-100,-4,-1,-0.5,-0.01,-0.0,1,Double.NaN,Double.NEGATIVE_INFINITY};
        int[] offsets = {-48,-12,-4,-1,0,1,4,12,48};
        for (int n=0;n<5000;n++) {
            int x=samples.nextInt(16), y=-64+samples.nextInt(128), z=samples.nextInt(16);
            double density=densities[samples.nextInt(densities.length)];
            mode[0]=samples.nextInt(5); here[0]=y;
            long[] locations=(long[])locationsField.get(aquifer);
            var fluids=(Aquifer.FluidStatus[])fluidsField.get(aquifer);
            int[] best={-1,-1,-1,-1}, dist={Integer.MAX_VALUE,Integer.MAX_VALUE,Integer.MAX_VALUE,Integer.MAX_VALUE};
            int bx=(x-5)>>4, by=Math.floorDiv(y+1,12), bz=(z-5)>>4;
            for(int dx=0;dx<=1;dx++) for(int dy=-1;dy<=1;dy++) for(int dz=0;dz<=1;dz++) {
                int gx=bx+dx, gy=by+dy, gz=bz+dz, i=(int)index.invoke(aquifer,gx,gy,gz);
                var r=factory.at(gx,gy,gz);
                long pos=BlockPos.asLong((gx<<4)+r.nextInt(10), gy*12+r.nextInt(9), (gz<<4)+r.nextInt(10));
                locations[i]=pos;
                fluids[i]=new Aquifer.FluidStatus(y+offsets[samples.nextInt(offsets.length)],state(samples.nextInt(4)));
                int a=BlockPos.getX(pos)-x,b=BlockPos.getY(pos)-y,c=BlockPos.getZ(pos)-z,d=a*a+b*b+c*c;
                for(int j=0;j<4;j++) if(d<=dist[j]) { for(int k=3;k>j;k--) {dist[k]=dist[k-1]; best[k]=best[k-1];} dist[j]=d; best[j]=i; break; }
            }
            int actual=code(aquifer.computeSubstance(x,y,z,density));
            boolean update=aquifer.shouldScheduleFluidUpdate();
            StringBuilder line=new StringBuilder("D "+y+" "+Double.doubleToRawLongBits(density)+" "+skipField.getInt(aquifer)+" "+mode[0]);
            for(int j=0;j<4;j++) line.append(' ').append(dist[j]).append(' ').append(fluids[best[j]].fluidLevel()).append(' ').append(code(fluids[best[j]].fluidType()));
            line.append(' ').append(actual).append(' ').append(update ? 1 : 0);
            out.println(line);
        }
    }
}
