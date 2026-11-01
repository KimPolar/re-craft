import com.mojang.blaze3d.vertex.PoseStack;
import com.mojang.blaze3d.vertex.VertexConsumer;
import net.minecraft.client.renderer.ScreenEffectRenderer;
import net.minecraft.client.renderer.state.level.PlayerRenderState.WaterOverlay;
import net.minecraft.util.ARGB;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;

public class WaterOverlayOracle {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        Method render = ScreenEffectRenderer.class.getDeclaredMethod("lambda$submitWater$0", WaterOverlay.class, PoseStack.Pose.class, VertexConsumer.class);
        render.setAccessible(true);
        try (var out = new java.io.PrintWriter(Files.newBufferedWriter(Path.of(args[0])))) {
            for (float yaw : new float[]{-180f, -25f, 0f, 75f, 360f}) for (float pitch : new float[]{-90f, 0f, 40f, 90f}) for (float brightness : new float[]{0f, 0.1f, 0.5f, 1f}) {
                int color = ARGB.colorFromFloat(0.1f, brightness, brightness, brightness);
                WaterOverlay state = new WaterOverlay(color, -yaw / 64.0f, pitch / 64.0f);
                float[] point = new float[5];
                VertexConsumer consumer = (VertexConsumer) Proxy.newProxyInstance(VertexConsumer.class.getClassLoader(), new Class<?>[]{VertexConsumer.class}, (proxy, method, values) -> {
                    if (method.getName().equals("addVertex") && values.length == 4) { point[0]=(float)values[1]; point[1]=(float)values[2]; }
                    if (method.getName().equals("setUv")) { point[2]=(float)values[0]; point[3]=(float)values[1]; }
                    if (method.getName().equals("setColor") && values.length == 1) { out.printf(Locale.ROOT, "%s %s %s %s %d%n", point[0], point[1], point[2], point[3], (int)values[0]); }
                    return proxy;
                });
                render.invoke(null, state, new PoseStack.Pose(), consumer);
            }
            for (float ambient : new float[]{0f, 0.1f, 1f}) {
                var dimension = new net.minecraft.world.level.dimension.DimensionType(false, true, false, false, 1.0, -16, 32, 32, null, ambient, null, null, null, null, null, Optional.empty());
                for (int level = 0; level < 16; level++) out.println(net.minecraft.client.renderer.Lightmap.getBrightness(dimension, level));
            }
        }
    }
}
