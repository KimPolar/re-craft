import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.client.server.IntegratedServer;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityDimensions;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.FarmlandBlock;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.block.state.BlockState;

public class FarmlandTrampleOracle {
    static class World extends ServerLevel {
        RandomSource rng;
        MinecraftServer server;
        World() { super(null, null, null, null, null, null, false, 0, List.of(), false); }
        @Override public RandomSource getRandom() { return rng; }
        @Override public MinecraftServer getServer() { return server; }
    }
    static class Probe extends Player {
        boolean damageQueried;
        Probe() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public net.minecraft.world.damagesource.DamageSources damageSources() { damageQueried = true; return null; }
    }
    static class Farmland extends FarmlandBlock {
        boolean changed;
        Farmland() { super(Blocks.DIRT, BlockBehaviour.Properties.of()); }
        @Override public void turnToBaseBlock(Entity entity, BlockState state, Level level, BlockPos pos) { changed = true; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        world.server = (IntegratedServer) unsafe.allocateInstance(IntegratedServer.class);
        var player = (Probe) unsafe.allocateInstance(Probe.class);
        var dimensions = Entity.class.getDeclaredField("dimensions");
        dimensions.setAccessible(true);
        var block = (Farmland) unsafe.allocateInstance(Farmland.class);
        var results = new JsonArray();
        for (long seed = 0; seed < 64; seed++) {
            for (float height : new float[]{1.8f, 1.5f, 0.6f}) {
                for (double distance : new double[]{0.01, 0.5, 0.75, 1.0, 1.5, 2.0}) {
                    dimensions.set(player, EntityDimensions.fixed(0.6f, height));
                    world.rng = RandomSource.create(seed);
                    block.changed = false;
                    player.damageQueried = false;
                    try {
                        block.fallOn(world, Blocks.FARMLAND.defaultBlockState(), BlockPos.ZERO, player, distance);
                        throw new AssertionError("expected omitted damage source");
                    } catch (NullPointerException expected) {
                        // The fall damage path follows the trample decision; it is tested elsewhere.
                        if (!player.damageQueried) throw expected;
                    }
                    var row = new JsonObject();
                    row.addProperty("seed", seed);
                    row.addProperty("height", height);
                    row.addProperty("distance", distance);
                    row.addProperty("changed", block.changed);
                    row.addProperty("next", world.rng.nextInt());
                    results.add(row);
                }
            }
        }
        Files.writeString(Path.of(args[0]), results.toString());
    }
}
