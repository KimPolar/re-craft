import com.mojang.blaze3d.vertex.VertexConsumer;
import net.minecraft.client.multiplayer.ClientLevel.ClientLevelData;
import net.minecraft.client.renderer.SkyRenderer;
import net.minecraft.world.Difficulty;
import net.minecraft.world.level.LevelHeightAccessor;
import sun.misc.Unsafe;
import java.lang.reflect.*;
import java.util.*;

public class DarkSkyOracle {
    public static void main(String[] args) throws Exception {
        Field f = Unsafe.class.getDeclaredField("theUnsafe");
        f.setAccessible(true);
        SkyRenderer sky = (SkyRenderer) ((Unsafe) f.get(null)).allocateInstance(SkyRenderer.class);
        List<float[]> points = new ArrayList<>();
        VertexConsumer consumer = (VertexConsumer) Proxy.newProxyInstance(VertexConsumer.class.getClassLoader(), new Class<?>[]{VertexConsumer.class}, (proxy, method, values) -> {
            if (method.getName().equals("addVertex")) points.add(new float[]{(float)values[0], (float)values[1], (float)values[2]});
            return proxy;
        });
        Method build = SkyRenderer.class.getDeclaredMethod("buildSkyDisc", VertexConsumer.class, float.class);
        build.setAccessible(true);
        for (float height : new float[]{-16.0f, 16.0f}) {
            points.clear();
            build.invoke(sky, consumer, height);
            for (float[] p : points) System.out.println(Float.floatToRawIntBits(p[0]) + " " + Float.floatToRawIntBits(p[1]) + " " + Float.floatToRawIntBits(p[2]));
        }
        for (boolean flat : new boolean[]{false, true}) {
            ClientLevelData data = new ClientLevelData(Difficulty.NORMAL, false, flat);
            for (int minY : new int[]{-128, -64, 0, 32}) {
                System.out.println(data.getHorizonHeight(LevelHeightAccessor.create(minY, 384)));
            }
        }
    }
}
