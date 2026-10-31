import java.io.PrintWriter;
import java.lang.reflect.Field;
import java.lang.reflect.Proxy;
import java.util.Random;
import net.minecraft.client.Camera;
import net.minecraft.client.DeltaTracker;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.client.renderer.fog.environment.AtmosphericFogEnvironment;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.world.level.LightLayer;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.lighting.LayerLightEventListener;
import net.minecraft.world.level.lighting.LevelLightEngine;
import sun.misc.Unsafe;

public class RainFogOracle {
    static final class FakeLevel extends ClientLevel {
        Holder<Biome> biome;
        LevelLightEngine light;
        float rain;
        FakeLevel() { super(null, null, null, null, 0, 0, null, false, 0, 0); }
        @Override public Holder<Biome> getBiome(BlockPos pos) { return biome; }
        @Override public LevelLightEngine getLightEngine() { return light; }
        @Override public float getRainLevel(float partial) { return rain; }
    }
    static final class FakeLight extends LevelLightEngine {
        LayerLightEventListener listener;
        FakeLight() { super(null, true, false); }
        @Override public LayerLightEventListener getLayerListener(LightLayer layer) { return listener; }
    }
    static final class FakeCamera extends Camera {
        @Override public BlockPos blockPosition() { return BlockPos.ZERO; }
    }
    static final class Delta implements DeltaTracker {
        float delta;
        public float getGameTimeDeltaTicks() { return delta; }
        public float getGameTimeDeltaPartialTick(boolean ignored) { return 0.5f; }
        public float getRealtimeDeltaTicks() { return delta; }
    }

    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        Field f = Unsafe.class.getDeclaredField("theUnsafe");
        f.setAccessible(true);
        var unsafe = (Unsafe) f.get(null);
        var level = (FakeLevel) unsafe.allocateInstance(FakeLevel.class);
        var light = (FakeLight) unsafe.allocateInstance(FakeLight.class);
        var camera = (FakeCamera) unsafe.allocateInstance(FakeCamera.class);
        level.light = light;
        var climateClass = Class.forName("net.minecraft.world.level.biome.Biome$ClimateSettings");
        var climateCtor = climateClass.getDeclaredConstructor(boolean.class, float.class, Biome.TemperatureModifier.class, float.class);
        climateCtor.setAccessible(true);
        var climateField = Biome.class.getDeclaredField("climateSettings");
        climateField.setAccessible(true);
        var biomes = new Holder[2];
        for (int i = 0; i < 2; i++) {
            var biome = (Biome) unsafe.allocateInstance(Biome.class);
            climateField.set(biome, climateCtor.newInstance(i != 0, 0.8f, Biome.TemperatureModifier.NONE, 0.4f));
            biomes[i] = Holder.direct(biome);
        }
        var env = new AtmosphericFogEnvironment();
        var update = AtmosphericFogEnvironment.class.getDeclaredMethod("updateRainFogState", Camera.class, ClientLevel.class, DeltaTracker.class);
        update.setAccessible(true);
        var multiplier = AtmosphericFogEnvironment.class.getDeclaredField("rainFogMultiplier");
        multiplier.setAccessible(true);
        var random = new Random(44923);
        var delta = new Delta();
        int[] levelValue = {0};
        light.listener = (LayerLightEventListener) Proxy.newProxyInstance(RainFogOracle.class.getClassLoader(), new Class[]{LayerLightEventListener.class}, (object, method, params) -> {
            if (method.getName().equals("getLightValue")) return levelValue[0];
            if (method.getReturnType() == boolean.class) return false;
            if (method.getReturnType() == int.class) return 0;
            return null;
        });
        try (var out = new PrintWriter(args[0])) {
            for (int i = 0; i < 5000; i++) {
                if (i % 100 == 0) multiplier.setFloat(env, 0f);
                level.rain = random.nextFloat();
                levelValue[0] = random.nextInt(16);
                int precipitation = random.nextInt(2);
                level.biome = biomes[precipitation];
                delta.delta = random.nextInt(101) / 50f;
                update.invoke(env, camera, level, delta);
                out.printf("%d %d %d %d %d%n", Float.floatToRawIntBits(level.rain), levelValue[0], precipitation, Float.floatToRawIntBits(delta.delta), Float.floatToRawIntBits(multiplier.getFloat(env)));
            }
        }
    }
}
