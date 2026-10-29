import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;

import com.google.gson.Gson;
import com.google.gson.JsonArray;
import net.minecraft.client.Minecraft;
import net.minecraft.client.renderer.GameRenderer;
import net.minecraft.client.renderer.rendertype.TextureTransform;
import net.minecraft.client.renderer.state.GameRenderState;
import net.minecraft.client.renderer.state.OptionsRenderState;
import net.minecraft.util.TimeSource;
import net.minecraft.util.Util;
import org.joml.Vector3f;
import sun.misc.Unsafe;

/** Calls the installed game's real item-glint transform without opening a game window. */
public class GlintTransformOracle {
    private static void setField(Class<?> type, Object instance, String name, Object value)
            throws Exception {
        Field field = type.getDeclaredField(name);
        field.setAccessible(true);
        field.set(instance, value);
    }

    public static void main(String[] args) throws Exception {
        Field unsafeField = Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        Unsafe unsafe = (Unsafe) unsafeField.get(null);
        Minecraft minecraft = (Minecraft) unsafe.allocateInstance(Minecraft.class);
        GameRenderer renderer = (GameRenderer) unsafe.allocateInstance(GameRenderer.class);
        GameRenderState state = (GameRenderState) unsafe.allocateInstance(GameRenderState.class);
        OptionsRenderState options = (OptionsRenderState) unsafe.allocateInstance(OptionsRenderState.class);
        setField(Minecraft.class, null, "instance", minecraft);
        setField(Minecraft.class, minecraft, "gameRenderer", renderer);
        setField(GameRenderer.class, renderer, "gameRenderState", state);
        setField(GameRenderState.class, state, "optionsRenderState", options);

        long[] times = {0, 1, 124, 12345, 29999, 30000, 137500, 3600000, 4294967295L};
        double[] speeds = {0.0, 0.1, 0.5, 0.75, 1.0};
        float[][] points = {{0f, 0f}, {0.11f, 0.57f}, {0.9f, 0.2f}, {1f, 1f}, {1.25f, -0.5f}};
        JsonArray out = new JsonArray();
        for (long millis : times) {
            Util.setTimeSource(TimeSource.constant(millis * 1_000_000));
            for (double speed : speeds) {
                options.glintSpeed = speed;
                var matrix = TextureTransform.GLINT_TEXTURING.createMatrix();
                for (float[] point : points) {
                    var result = matrix.transformPosition(new Vector3f(point[0], point[1], 0));
                    JsonArray xy = new JsonArray();
                    xy.add(result.x);
                    xy.add(result.y);
                    out.add(xy);
                }
            }
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(out));
    }
}
