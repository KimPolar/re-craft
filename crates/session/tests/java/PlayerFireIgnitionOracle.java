import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.BaseFireBlock;

public class PlayerFireIgnitionOracle {
    static class World extends ServerLevel {
        RandomSource random;
        World() { super(null,null,null,null,null,null,false,0L,null,false); }
        @Override public RandomSource getRandom() { return random; }
    }
    static class TestPlayer extends ServerPlayer {
        World world;
        double burningTime;
        TestPlayer() { super(null, null, null, null); }
        @Override public ServerLevel level() { return world; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) { return burningTime; }
        @Override public boolean fireImmune() { return false; }
        @Override public void clearFreeze() { }
    }

    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var player = (TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        var world = (World)unsafe.allocateInstance(World.class);
        player.world = world;
        var abilities = new net.minecraft.world.entity.player.Abilities();
        var abilityField = Player.class.getDeclaredField("abilities");
        unsafe.putObject(player, unsafe.objectFieldOffset(abilityField), abilities);
        var fireField = net.minecraft.world.entity.Entity.class.getDeclaredField("remainingFireTicks");
        fireField.setAccessible(true);
        var rows = new JsonArray();
        for (boolean invulnerable : new boolean[]{false, true}) {
            abilities.invulnerable = invulnerable;
            for (double multiplier : new double[]{0.0, 0.1, 0.5, 1.0, 2.0}) {
                for (int previous : new int[]{-21, -20, -2, -1, 0, 1, 100, 160, 300}) {
                    for (long seed : new long[]{0L, 1L, 1234567L}) {
                        world.random = RandomSource.create(seed);
                        player.burningTime = multiplier;
                        fireField.setInt(player, previous);
                        BaseFireBlock.fireIgnite(player);
                        var row = new JsonObject();
                        row.addProperty("invulnerable", invulnerable);
                        row.addProperty("multiplier", multiplier);
                        row.addProperty("previous", previous);
                        row.addProperty("seed", seed);
                        row.addProperty("result", player.getRemainingFireTicks());
                        row.addProperty("next", world.random.nextInt());
                        rows.add(row);
                    }
                }
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(rows));
    }
}
