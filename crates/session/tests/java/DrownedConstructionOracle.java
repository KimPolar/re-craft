import java.util.concurrent.atomic.AtomicLong;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.ai.navigation.AmphibiousPathNavigation;
import net.minecraft.world.entity.ai.navigation.PathNavigation;
import net.minecraft.world.entity.monster.zombie.Drowned;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.levelgen.LegacyRandomSource;

public class DrownedConstructionOracle {
    static class World extends StructureDrownedOracle.World {
        @Override public int getSeaLevel() { return 63; }
    }
    static class DrownedWithNavigation extends Drowned {
        DrownedWithNavigation(World world) { super(EntityTypes.DROWNED, world); }
        // Keep the real goal registration and all entity constructors.
        @Override protected PathNavigation createNavigation(Level level) {
            try { return (PathNavigation) StructureDrownedOracle.allocate.invoke(
                    StructureDrownedOracle.unsafe, AmphibiousPathNavigation.class); }
            catch (ReflectiveOperationException error) { throw new RuntimeException(error); }
        }
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var unsafeClass = Class.forName("sun.misc.Unsafe");
        var unsafeField = unsafeClass.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        StructureDrownedOracle.unsafe = unsafeField.get(null);
        StructureDrownedOracle.allocate = unsafeClass.getMethod("allocateInstance", Class.class);
        var world = (World) StructureDrownedOracle.allocate.invoke(StructureDrownedOracle.unsafe, World.class);
        var randomField = Entity.class.getDeclaredField("random"); randomField.setAccessible(true);
        var seedField = LegacyRandomSource.class.getDeclaredField("seed"); seedField.setAccessible(true);
        for (int i = 0; i < 256; i++) {
            var drowned = new DrownedWithNavigation(world);
            var random = (LegacyRandomSource) randomField.get(drowned);
            long state = ((AtomicLong) seedField.get(random)).get();
            long original = state;
            for (int draw = 0; draw < 5; draw++)
                original = ((original - 11) * 246154705703781L) & ((1L << 48) - 1);
            long seed = original ^ 0x5deece66dL;
            var uuid = drowned.getUUID();
            out.println("C\t" + seed + "\t" + uuid.getMostSignificantBits() + "\t"
                    + uuid.getLeastSignificantBits() + "\t" + Float.floatToIntBits(drowned.getYRot())
                    + "\t" + random.nextLong());
        }
    }
}
