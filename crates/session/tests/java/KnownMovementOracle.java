import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientPacketListener;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.client.Minecraft;
import net.minecraft.client.Options;
import net.minecraft.client.OptionInstance;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.game.ServerboundMovePlayerPacket;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.phys.Vec3;

public class KnownMovementOracle {
    static class Connection extends ClientPacketListener {
        ServerboundMovePlayerPacket packet;
        Connection() { super(null, null, null); }
        @Override public void send(Packet<?> value) {
            if (value instanceof ServerboundMovePlayerPacket move) packet = move;
        }
    }
    static class Player extends LocalPlayer {
        float yaw;
        boolean ground;
        Player() { super(null, null, null, null, null, null, false, null, null); }
        @Override protected boolean isControlledCamera() { return true; }
        @Override public boolean isSprinting() { return false; }
        @Override public boolean onGround() { return ground; }
        @Override public float getYRot() { return yaw; }
        @Override public float getXRot() { return 0; }
    }
    static JsonArray bits(Vec3 value) {
        var result = new JsonArray();
        result.add(Double.doubleToRawLongBits(value.x));
        result.add(Double.doubleToRawLongBits(value.y));
        result.add(Double.doubleToRawLongBits(value.z));
        return result;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) f.get(null);
        Field connectionField = LocalPlayer.class.getDeclaredField("connection"); connectionField.setAccessible(true);
        Field positionField = Entity.class.getDeclaredField("position"); positionField.setAccessible(true);
        Field minecraftField = LocalPlayer.class.getDeclaredField("minecraft"); minecraftField.setAccessible(true);
        Field optionsField = Minecraft.class.getDeclaredField("options"); optionsField.setAccessible(true);
        Field autoJumpField = Options.class.getDeclaredField("autoJump"); autoJumpField.setAccessible(true);
        Field valueField = OptionInstance.class.getDeclaredField("value"); valueField.setAccessible(true);
        var minecraft = (Minecraft) unsafe.allocateInstance(Minecraft.class);
        var options = (Options) unsafe.allocateInstance(Options.class);
        var autoJump = (OptionInstance<?>) unsafe.allocateInstance(OptionInstance.class);
        valueField.set(autoJump, false); autoJumpField.set(options, autoJump); optionsField.set(minecraft, options);
        Method send = LocalPlayer.class.getDeclaredMethod("sendPosition"); send.setAccessible(true);
        var rows = new JsonArray();
        for (int scenario = 0; scenario < 6; scenario++) {
            var player = (Player) unsafe.allocateInstance(Player.class);
            var connection = (Connection) unsafe.allocateInstance(Connection.class);
            connectionField.set(player, connection);
            minecraftField.set(player, minecraft);
            double base = switch (scenario) { case 0 -> 0; case 1 -> 3.5; case 2 -> -30000000; default -> scenario * 100.0; };
            var position = new Vec3(base, scenario == 0 ? 0 : 64, -base);
            var serverPosition = position;
            var steps = new JsonArray();
            for (int tick = 0; tick < 120; tick++) {
                double dx = switch (tick % 12) { case 0, 1, 2 -> 0.0001; case 3 -> -0.0002; case 4 -> 0.2; case 5 -> -0.2; default -> 0; };
                double dy = tick % 19 == 0 ? 0.00015 : 0;
                double dz = tick % 23 == 0 ? -0.00017 : 0;
                if (tick >= 60 && tick < 100) { dx = 0; dy = 0; dz = 0; }
                position = position.add(dx, dy, dz);
                positionField.set(player, position);
                player.ground = tick % 7 < 4;
                player.yaw = tick % 5 == 0 ? tick : player.yaw;
                player.horizontalCollision = tick % 11 == 0;
                connection.packet = null;
                send.invoke(player);
                Vec3 known = Vec3.ZERO;
                if (connection.packet != null && connection.packet.hasPosition()) {
                    var reported = new Vec3(connection.packet.getX(serverPosition.x), connection.packet.getY(serverPosition.y), connection.packet.getZ(serverPosition.z));
                    known = reported.subtract(serverPosition);
                    serverPosition = reported;
                }
                var step = new JsonObject();
                step.add("position", bits(position)); step.add("known", bits(known));
                step.addProperty("reported", connection.packet != null && connection.packet.hasPosition());
                steps.add(step);
            }
            var row = new JsonObject(); row.addProperty("base", base); row.add("steps", steps); rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
