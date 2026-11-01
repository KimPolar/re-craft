import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.sounds.SoundSource;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.Vec3;

public class HoneySlideOracle {
    static class TestLevel extends ServerLevel {
        RandomSource random;
        int sounds, particles;
        boolean client;
        TestLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public RandomSource getRandom() { return random; }
        @Override public boolean isClientSide() { return client; }
        @Override public void broadcastEntityEvent(Entity entity, byte event) { if (event == 53) particles++; }
        @Override public void playSound(Entity source, double x, double y, double z, SoundEvent event, SoundSource category, float volume, float pitch) {
            if (category != SoundSource.PLAYERS || volume != 1.0f || pitch != 1.0f) throw new IllegalStateException("sound parameters");
            sounds++;
        }
    }
    static class TestPlayer extends Player {
        boolean grounded, silent;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean onGround() { return grounded; }
        @Override public boolean isSilent() { return silent; }
        @Override public SoundSource getSoundSource() { return SoundSource.PLAYERS; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)f.get(null);
        var player = (TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        var world = (TestLevel)unsafe.allocateInstance(TestLevel.class);
        Field level = Entity.class.getDeclaredField("level"); level.setAccessible(true); level.set(player, world);
        Field position = Entity.class.getDeclaredField("position"); position.setAccessible(true);
        Field dimensions = Entity.class.getDeclaredField("dimensions"); dimensions.setAccessible(true);
        dimensions.set(player, EntityDimensions.scalable(0.6f, 1.8f));
        Method inside = HoneyBlock.class.getDeclaredMethod("entityInside", BlockState.class, Level.class, BlockPos.class, Entity.class, InsideBlockEffectApplier.class, boolean.class);
        inside.setAccessible(true);
        var rows = new JsonArray();
        for (var block : new BlockPos[]{new BlockPos(10,20,-30), new BlockPos(100000000,-32,-100000000)})
            for (double offset : new double[]{0.5, 0.4375 + (double)(0.6f / 2f) - 0.000001,
                    0.4375 + (double)(0.6f / 2f), 0.4375 + (double)(0.6f / 2f) + 0.000001})
                for (int axis : new int[]{0,1}) for (double yOffset : new double[]{0.2,0.9375 - 1.0e-7,0.9375})
                    for (double old : new double[]{-0.06,-0.08,-0.080001,-0.13,-0.130001,-0.2,-1.0})
                        for (boolean ground : new boolean[]{false,true}) for (boolean silent : new boolean[]{false,true}) for (boolean client : new boolean[]{false,true})
                            for (long seed : new long[]{0,1,17,1234567}) {
                                double x = block.getX() + 0.5 + (axis == 0 ? offset : 0);
                                double z = block.getZ() + 0.5 + (axis == 1 ? offset : 0);
                                double y = block.getY() + yOffset;
                                var initial = new Vec3(0.4, (old - 0.08) * (double)0.98f, -0.2);
                                position.set(player, new Vec3(x,y,z)); player.setDeltaMovement(initial);
                                player.grounded=ground; player.silent=silent; player.fallDistance=9.0;
                                world.random=RandomSource.create(seed); world.sounds=0; world.particles=0; world.client=client;
                                inside.invoke(Blocks.HONEY_BLOCK, Blocks.HONEY_BLOCK.defaultBlockState(), world, block, player, null, false);
                                var row = new JsonObject(); var pos = new JsonArray(); pos.add(x); pos.add(y); pos.add(z); row.add("position",pos);
                                var blockPos = new JsonArray(); blockPos.add(block.getX()); blockPos.add(block.getY()); blockPos.add(block.getZ()); row.add("block",blockPos);
                                var input = new JsonArray(); input.add(initial.x); input.add(initial.y); input.add(initial.z); row.add("initial",input);
                                var velocity = player.getDeltaMovement(); var output = new JsonArray(); output.add(velocity.x); output.add(velocity.y); output.add(velocity.z); row.add("velocity",output);
                                row.addProperty("ground",ground); row.addProperty("silent",silent); row.addProperty("client",client); row.addProperty("seed",seed); row.addProperty("fall",player.fallDistance);
                                row.addProperty("sounds",world.sounds); row.addProperty("particles",world.particles); row.addProperty("next",world.random.nextInt()); rows.add(row);
                            }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
