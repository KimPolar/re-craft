import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.HumanoidArm;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.Vec3;

public class ClimbableVelocityOracle {
    static class TestPlayer extends Player {
        boolean climbing, suppress, scaffolding;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public HumanoidArm getMainArm() { return HumanoidArm.RIGHT; }
        @Override public boolean onClimbable() { return climbing; }
        @Override public boolean isSuppressingSlidingDownLadder() { return suppress; }
        @Override public BlockState getInBlockState() {
            return (scaffolding ? Blocks.SCAFFOLDING : Blocks.LADDER).defaultBlockState();
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        Field unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        Method method = LivingEntity.class.getDeclaredMethod("handleOnClimbable", Vec3.class);
        method.setAccessible(true);
        var rows = new JsonArray();
        double[] values = {-1.0, -0.15000000596046448, -0.15, -0.01, 0.0, 0.01, 0.15, 0.15000000596046448, 1.0};
        for (boolean climbing : new boolean[]{false, true})
            for (boolean suppress : new boolean[]{false, true})
                for (boolean scaffolding : new boolean[]{false, true})
                    for (double x : values)
                        for (double y : values) {
                            double z = -x;
                            player.climbing = climbing;
                            player.suppress = suppress;
                            player.scaffolding = scaffolding;
                            player.fallDistance = 23.5;
                            Vec3 result = (Vec3) method.invoke(player, new Vec3(x, y, z));
                            var row = new JsonObject();
                            row.addProperty("climbing", climbing);
                            row.addProperty("suppress", suppress);
                            row.addProperty("scaffolding", scaffolding);
                            row.addProperty("x", x);
                            row.addProperty("y", y);
                            row.addProperty("z", z);
                            row.addProperty("outX", Long.toUnsignedString(Double.doubleToRawLongBits(result.x)));
                            row.addProperty("outY", Long.toUnsignedString(Double.doubleToRawLongBits(result.y)));
                            row.addProperty("outZ", Long.toUnsignedString(Double.doubleToRawLongBits(result.z)));
                            row.addProperty("fall", player.fallDistance);
                            rows.add(row);
                        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
