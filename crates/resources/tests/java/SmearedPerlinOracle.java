import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.synth.SmearedPerlinNoise;
import net.minecraft.world.level.levelgen.densityfunction.DensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;

public class SmearedPerlinOracle {
    public static void main(String[] args) {
        long[] seeds = {0, 1, -1, 2345, -987654321, Long.MIN_VALUE, Long.MAX_VALUE};
        double[] smears = {0.0005, 0.25, 1.0, 8.0, 684.412};
        for (int mode=0;mode<2;mode++) for (long seed:seeds) for (double smear:smears) {
            RandomSource random = mode == 0 ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
            var noise = new SmearedPerlinNoise(random, smear);
            System.out.printf("S %d %d %d %d%n",mode,seed,Double.doubleToRawLongBits(smear),random.nextLong());
            for (int i=-32;i<32;i++) {
                double x=i*13.0/7.0, y=i*19.0/11.0, z=i*-37.0/5.0;
                System.out.printf("P %d %d %d %d%n",Double.doubleToRawLongBits(x),Double.doubleToRawLongBits(y),Double.doubleToRawLongBits(z),Float.floatToRawIntBits(noise.get(x,y,z)));
            }
            double xzScale=8.55515, yScale=4.277575;
            var volume = new DensityVolume(3,4,2,-17,-5,31,4,8,4);
            var buffer = DensityBuffer.createUnpooled(volume.size());
            noise.addToVolume(buffer,volume,xzScale,yScale,1.0f);
            for (int z=0;z<volume.sizeZ();z++) for (int x=0;x<volume.sizeX();x++) for (int y=0;y<volume.sizeY();y++) {
                int bx=volume.blockX(x), by=volume.blockY(y), bz=volume.blockZ(z);
                System.out.printf("V %d %d %d %d %d %d%n",bx,by,bz,Double.doubleToRawLongBits(xzScale),Double.doubleToRawLongBits(yScale),Float.floatToRawIntBits(buffer.get(volume.indexUnchecked(x,y,z))));
            }
        }
    }
}
