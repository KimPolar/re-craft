import net.minecraft.client.Camera;
import net.minecraft.client.renderer.fog.FogData;
import net.minecraft.client.renderer.fog.environment.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.damagesource.DamageSource;
import net.minecraft.world.level.storage.ValueInput;
import net.minecraft.world.level.storage.ValueOutput;
import org.joml.Vector3fc;
import sun.misc.Unsafe;
import java.lang.reflect.Field;

public class SpecialFogOracle {
    static class Dummy extends Entity {
        boolean spectator;
        Dummy() { super(null, null); }
        public boolean isSpectator() { return spectator; }
        protected void defineSynchedData(SynchedEntityData.Builder builder) {}
        public boolean hurtServer(ServerLevel level, DamageSource source, float amount) { return false; }
        protected void readAdditionalSaveData(ValueInput input) {}
        protected void addAdditionalSaveData(ValueOutput output) {}
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        Field f = Unsafe.class.getDeclaredField("theUnsafe");
        f.setAccessible(true);
        Unsafe unsafe = (Unsafe) f.get(null);
        Camera camera = (Camera) unsafe.allocateInstance(Camera.class);
        Dummy entity = (Dummy) unsafe.allocateInstance(Dummy.class);
        Field entityField = Camera.class.getDeclaredField("entity");
        entityField.setAccessible(true);
        entityField.set(camera, entity);
        try (var out = new java.io.PrintWriter(java.nio.file.Files.newBufferedWriter(java.nio.file.Path.of(args[0])))) {
        for (FogEnvironment env : new FogEnvironment[]{new LavaFogEnvironment(), new PowderedSnowFogEnvironment()}) {
            Vector3fc color = env.getBaseColor(null, camera, 0, 0.0f);
            for (boolean spectator : new boolean[]{false, true}) {
                entity.spectator = spectator;
                for (int chunks : new int[]{2, 8, 12, 32}) {
                    FogData data = new FogData();
                    env.setupFog(data, camera, null, (float)(chunks * 16), null);
                    out.printf(java.util.Locale.ROOT, "%s %s %s %s %s %s%n", color.x(), color.y(), color.z(), data.environmentalStart, data.environmentalEnd, data.skyEnd);
                }
            }
        }
        }
    }
}
