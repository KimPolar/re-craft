import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.*;

public class RedstoneOreOracle {
    static class TestLevel extends ServerLevel {
        BlockState current;
        RandomSource rng;
        BlockPos center;
        TestLevel() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public BlockState getBlockState(BlockPos pos) { return pos.equals(center) ? current : Blocks.AIR.defaultBlockState(); }
        @Override public boolean setBlockAndUpdate(BlockPos pos, BlockState state) { current = state; return true; }
        @Override public RandomSource getRandom() { return rng; }
        @Override public void addParticle(net.minecraft.core.particles.ParticleOptions p,double x,double y,double z,double vx,double vy,double vz) {}
    }
    static class TestPlayer extends Player {
        boolean careful;
        TestPlayer() { super(null,null); }
        @Override public net.minecraft.world.level.GameType gameMode() { return net.minecraft.world.level.GameType.SURVIVAL; }
        @Override public boolean isSteppingCarefully() { return careful; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field u = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); u.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)u.get(null);
        var world = (TestLevel)unsafe.allocateInstance(TestLevel.class);
        var player = (TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        world.center = BlockPos.ZERO;
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var entry : input) {
            var c = entry.getAsJsonObject();
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(c.get("block").getAsString()));
            world.current = block.defaultBlockState().setValue(RedStoneOreBlock.LIT, c.get("lit").getAsBoolean());
            world.rng = RandomSource.create(123);
            player.careful = c.get("action").getAsString().equals("sneak");
            switch (c.get("action").getAsString()) {
                case "attack" -> world.current.attack(world, world.center, player);
                case "step", "sneak" -> block.stepOn(world, world.center, world.current, player);
                case "random" -> world.current.randomTick(world, world.center, world.rng);
                case "use" -> world.current.useItemOn(ItemStack.EMPTY, world, player, InteractionHand.MAIN_HAND, new BlockHitResult(Vec3.ZERO, Direction.UP, world.center, false));
                default -> throw new IllegalArgumentException();
            }
            output.add(world.current.getValue(RedStoneOreBlock.LIT));
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
