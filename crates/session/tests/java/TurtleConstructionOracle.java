import java.util.List;
import java.util.concurrent.atomic.AtomicLong;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.ai.navigation.PathNavigation;
import net.minecraft.world.entity.animal.turtle.Turtle;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import sun.misc.Unsafe;

public class TurtleConstructionOracle {
    static Unsafe unsafe;
    static class World extends ServerLevel {
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public int getNextEntityId() { return 1; }
        @Override public int getSeaLevel() { return 63; }
    }
    static class TestTurtle extends Turtle {
        TestTurtle(World world) { super(EntityTypes.TURTLE, world); }
        @Override protected PathNavigation createNavigation(Level level) {
            try {
                return (PathNavigation) unsafe.allocateInstance(Class.forName(
                    "net.minecraft.world.entity.animal.turtle.Turtle$TurtlePathNavigation"));
            } catch (ReflectiveOperationException error) { throw new RuntimeException(error); }
        }
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var unsafeField = Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        unsafe = (Unsafe) unsafeField.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        var randomField = Entity.class.getDeclaredField("random"); randomField.setAccessible(true);
        var seedField = LegacyRandomSource.class.getDeclaredField("seed"); seedField.setAccessible(true);
        for (int i = 0; i < 256; i++) {
            var turtle = new TestTurtle(world);
            var random = (LegacyRandomSource) randomField.get(turtle);
            long original = ((AtomicLong) seedField.get(random)).get();
            for (int draw = 0; draw < 5; draw++)
                original = ((original - 11) * 246154705703781L) & ((1L << 48) - 1);
            long seed = original ^ 0x5deece66dL;
            var uuid = turtle.getUUID();
            out.println("C\t" + seed + "\t" + uuid.getMostSignificantBits() + "\t"
                + uuid.getLeastSignificantBits() + "\t" + Float.floatToIntBits(turtle.getYRot())
                + "\t" + random.nextLong());
        }
    }
}
