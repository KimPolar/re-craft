import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import com.google.gson.JsonArray;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.damagesource.DamageSource;
import net.minecraft.world.damagesource.DamageSources;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.block.Blocks;

public class MagmaStepOracle {
    static class TestLevel extends ServerLevel {
        DamageSources sources;
        TestLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public DamageSources damageSources() { return sources; }
    }
    static class TestPlayer extends Player {
        boolean careful;
        int hits;
        float amount;
        TestPlayer() { super(null, null); }
        @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
        @Override public boolean isSteppingCarefully() { return careful; }
        @Override public boolean hurtServer(ServerLevel level, DamageSource source, float value) {
            hits++; amount = value; return true;
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var level = (TestLevel)unsafe.allocateInstance(TestLevel.class);
        level.sources = (DamageSources)unsafe.allocateInstance(DamageSources.class);
        var player = (TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        Field entityLevel = Entity.class.getDeclaredField("level"); entityLevel.setAccessible(true); entityLevel.set(player, level);
        var results = new JsonArray();
        for (boolean careful : new boolean[]{false, true}) {
            player.careful = careful; player.hits = 0; player.amount = 0;
            Blocks.MAGMA_BLOCK.stepOn(level, BlockPos.ZERO, Blocks.MAGMA_BLOCK.defaultBlockState(), player);
            var row = new JsonArray(); row.add(player.hits); row.add(player.amount); results.add(row);
        }
        Files.writeString(Path.of(args[0]), results.toString());
    }
}
