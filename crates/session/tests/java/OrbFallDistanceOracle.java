import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.BlockPos;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.level.block.Blocks;

public class OrbFallDistanceOracle {
    static class Orb extends ExperienceOrb {
        boolean water;
        Orb() { super(null, 0, 0, 0, 1); }
        @Override public boolean isInWater() { return water; }
        void apply(double movement, boolean grounded) {
            checkFallDamage(movement, grounded, Blocks.AIR.defaultBlockState(), BlockPos.ZERO);
        }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        var f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) f.get(null);
        var orb = (Orb) unsafe.allocateInstance(Orb.class);
        var rows = new JsonArray();
        for (boolean water : new boolean[]{false, true})
            for (double initial : new double[]{0, 0.5, 5, 100000000})
                for (double movement : new double[]{-3.141592653589793, -0.2, -1e-12, 0, 0.2}) {
                    orb.water = water; orb.fallDistance = initial;
                    orb.apply(movement, false);
                    var row = new JsonObject();
                    row.addProperty("water", water); row.addProperty("initial", initial);
                    row.addProperty("movement", movement);
                    row.addProperty("fall_bits", Double.doubleToRawLongBits(orb.fallDistance));
                    rows.add(row);
                }
        orb.water = false; orb.fallDistance = 0; orb.apply(0.2, true);
        if (orb.fallDistance != 0) throw new AssertionError("ground reset");
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
