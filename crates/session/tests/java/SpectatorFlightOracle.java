import java.nio.file.Files;
import java.nio.file.Path;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.MoverType;
import net.minecraft.world.entity.ai.attributes.Attribute;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;
import net.minecraft.world.phys.Vec2;
import net.minecraft.world.phys.Vec3;

public class SpectatorFlightOracle {
    static class World extends ClientLevel {
        World() { super(null, null, null, null, 0, 0, null, false, 0, 0); }
        @Override public boolean isClientSide() { return false; }
    }
    static class Probe extends Player {
        Vec3 velocity = Vec3.ZERO, position = Vec3.ZERO;
        float yaw;
        boolean sprint;
        Probe() { super(null, null); }
        void setWorld(World world) { setLevel(world); }
        @Override public GameType gameMode() { return GameType.SPECTATOR; }
        @Override public boolean onGround() { return false; }
        @Override public Vec3 getDeltaMovement() { return velocity; }
        @Override public void setDeltaMovement(Vec3 value) { velocity = value; }
        @Override public void setDeltaMovement(double x, double y, double z) { velocity = new Vec3(x,y,z); }
        @Override public float getYRot() { return yaw; }
        @Override public boolean isSprinting() { return sprint; }
        @Override public boolean onClimbable() { return false; }
        @Override public boolean shouldDiscardFriction() { return false; }
        @Override protected boolean omnidirectionalAirMover() { return false; }
        @Override protected double getEffectiveGravity() { return 0.08; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) { return 1.0; }
        @Override public MobEffectInstance getEffect(Holder<MobEffect> effect) { return null; }
        @Override public BlockPos getBlockPosBelowThatAffectsMyMovement() { return BlockPos.ZERO; }
        @Override public void move(MoverType type, Vec3 requested) { position = position.add(requested); }
    }
    static class Input extends LocalPlayer {
        Input() { super(null, null, null, null, null, null, false, null, null); }
        @Override public boolean isMovingSlowly() { return false; }
        @Override public boolean isUsingItem() { return false; }
    }
    static JsonArray vec(Vec3 value) { var out = new JsonArray(); out.add(value.x); out.add(value.y); out.add(value.z); return out; }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) f.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        var player = (Probe) unsafe.allocateInstance(Probe.class);
        var input = (Input) unsafe.allocateInstance(Input.class);
        player.setWorld(world);
        Field abilities = Player.class.getDeclaredField("abilities"); abilities.setAccessible(true);
        var fly = new Abilities(); fly.flying = true; abilities.set(player, fly);
        Method travel = LivingEntity.class.getDeclaredMethod("travelInAir", Vec3.class); travel.setAccessible(true);
        Method modify = LocalPlayer.class.getDeclaredMethod("modifyInput", Vec2.class); modify.setAccessible(true);
        var rows = new JsonArray();
        for (int seed = 0; seed < 16; seed++) {
            player.position = Vec3.ZERO;
            player.velocity = new Vec3(seed * 0.001, -0.02, 0.03);
            for (int tick = 0; tick < 64; tick++) {
                float forward = (tick + seed) % 3 - 1, right = (tick / 3 + seed) % 3 - 1;
                boolean jump = (tick + seed) % 5 == 0, down = (tick + seed) % 7 == 0;
                player.sprint = (tick / 8 + seed) % 2 == 0;
                player.yaw = (tick * 37 + seed * 19) % 360 - 180;
                player.velocity = player.velocity.add(0, ((jump ? 1 : 0) - (down ? 1 : 0)) * fly.getFlyingSpeed() * 3.0f, 0);
                double x = player.velocity.x, y = player.velocity.y, z = player.velocity.z;
                if (x*x + z*z < 9.0e-6) { x = 0; z = 0; }
                if (Math.abs(y) < 0.003) y = 0;
                player.velocity = new Vec3(x,y,z);
                double oldY = y;
                Vec2 key = new Vec2(-right, forward).normalized();
                Vec2 adjusted = (Vec2) modify.invoke(input, key);
                travel.invoke(player, new Vec3(adjusted.x, 0, adjusted.y));
                player.velocity = new Vec3(player.velocity.x, oldY * 0.6, player.velocity.z);
                var row = new JsonObject();
                row.addProperty("seed", seed); row.addProperty("tick", tick);
                row.addProperty("forward", forward); row.addProperty("right", right);
                row.addProperty("jump", jump); row.addProperty("down", down);
                row.addProperty("sprint", player.sprint); row.addProperty("yaw", player.yaw);
                row.add("position", vec(player.position)); row.add("velocity", vec(player.velocity));
                rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
