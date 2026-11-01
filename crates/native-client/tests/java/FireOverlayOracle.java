import com.mojang.blaze3d.vertex.PoseStack;
import com.mojang.blaze3d.vertex.VertexConsumer;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.client.renderer.ScreenEffectRenderer;
import net.minecraft.client.renderer.texture.TextureAtlasSprite;
import org.joml.Matrix4fc;
import org.joml.Vector3f;

public class FireOverlayOracle {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var sprite = (TextureAtlasSprite) unsafe.allocateInstance(TextureAtlasSprite.class);
        for (var field : new String[] {"u0", "v0", "u1", "v1"}) {
            var target = TextureAtlasSprite.class.getDeclaredField(field);
            target.setAccessible(true);
            target.setFloat(sprite, field.endsWith("0") ? 0f : 1f);
        }
        var render = ScreenEffectRenderer.class.getDeclaredMethod(
            "lambda$submitFire$0", TextureAtlasSprite.class, PoseStack.Pose.class, VertexConsumer.class);
        render.setAccessible(true);
        try (var out = new java.io.PrintWriter(Files.newBufferedWriter(Path.of(args[0])))) {
            float[] point = new float[5];
            VertexConsumer consumer = (VertexConsumer) Proxy.newProxyInstance(
                VertexConsumer.class.getClassLoader(), new Class<?>[]{VertexConsumer.class},
                (proxy, method, values) -> {
                    if (method.getName().equals("addVertex") && values.length == 4) {
                        var transformed = ((Matrix4fc) values[0]).transformPosition(
                            (float)values[1], (float)values[2], (float)values[3], new Vector3f());
                        point[0] = transformed.x; point[1] = transformed.y; point[2] = transformed.z;
                    }
                    if (method.getName().equals("setUv")) {
                        point[3] = (float) values[0]; point[4] = (float) values[1];
                    }
                    if (method.getName().equals("setColor") && values.length == 1) {
                        out.printf(Locale.ROOT, "%s %s %s %s %s %d%n", point[0], point[1], point[2], point[3], point[4], (int) values[0]);
                    }
                    return proxy;
                });
            render.invoke(null, sprite, new PoseStack.Pose(), consumer);
        }
    }
}
