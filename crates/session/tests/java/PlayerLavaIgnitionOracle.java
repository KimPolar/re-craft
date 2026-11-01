import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.player.Player;

public class PlayerLavaIgnitionOracle {
    static final class TestPlayer extends Player {
        double burningTime;
        TestPlayer() { super(null, null); }
        @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) { return burningTime; }
        @Override public boolean fireImmune() { return false; }
        @Override public void clearFreeze() { }
    }

    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        var abilities = new net.minecraft.world.entity.player.Abilities();
        var abilityField = Player.class.getDeclaredField("abilities");
        unsafe.putObject(player, unsafe.objectFieldOffset(abilityField), abilities);
        var fireField = net.minecraft.world.entity.Entity.class.getDeclaredField("remainingFireTicks");
        fireField.setAccessible(true);
        var rows = new JsonArray();
        for (boolean invulnerable : new boolean[] {false, true}) {
        abilities.invulnerable = invulnerable;
        for (double multiplier : new double[] {0.0, 0.01, 0.1, 1.0 / 3.0, 0.5, 1.0, 2.0, 1024.0}) {
            for (int previous : new int[] {-20, 0, 1, 100, 300, 500}) {
                player.burningTime = multiplier;
                fireField.setInt(player, previous);
                player.lavaIgnite();
                var row = new JsonObject();
                row.addProperty("invulnerable", invulnerable);
                row.addProperty("multiplier", multiplier);
                row.addProperty("previous", previous);
                row.addProperty("result", player.getRemainingFireTicks());
                rows.add(row);
            }
        }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
    }
}
