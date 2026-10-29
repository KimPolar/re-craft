import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.function.Consumer;

import com.google.gson.Gson;
import com.google.gson.JsonArray;
import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.renderer.SubmitNodeCollector;
import net.minecraft.client.renderer.entity.ItemEntityRenderer;
import net.minecraft.client.renderer.entity.state.ItemClusterRenderState;
import net.minecraft.client.renderer.special.SpecialModelRenderer;
import net.minecraft.util.RandomSource;
import net.minecraft.world.item.ItemDisplayContext;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.phys.AABB;
import org.joml.Vector3f;
import org.joml.Vector3fc;

/** Captures the real 26.3 renderer's model submissions without a window. */
public class GroundItemOracle {
    static class Capture implements SpecialModelRenderer<Object> {
        JsonArray offsets = new JsonArray();
        public void submit(Object arg, PoseStack pose, SubmitNodeCollector collector,
                int light, int overlay, boolean foil, int outline) {
            // ItemTransform also translates the model origin by -0.5.
            Vector3f point = pose.last().pose().transformPosition(new Vector3f(0.5f, 0.5f, 0.5f));
            JsonArray row = new JsonArray();
            row.add(point.x); row.add(point.y); row.add(point.z);
            offsets.add(row);
        }
        public void getExtents(Consumer<Vector3fc> consumer) {}
        public Object extractArgument(ItemStack stack) { return null; }
    }

    public static void main(String[] args) throws Exception {
        int[] seeds = {0, 1, -1, 187, 1337, Integer.MAX_VALUE};
        int[] counts = {1, 2, 16, 17, 32, 33, 48, 49, 64};
        float[] depths = {0f, 0.03125f, 0.0625f, 0.0625001f, 0.25f, 1f};
        Field context = net.minecraft.client.renderer.item.ItemStackRenderState.class
                .getDeclaredField("displayContext");
        context.setAccessible(true);
        JsonArray out = new JsonArray();
        for (int seed : seeds) for (int count : counts) for (float depth : depths) {
            ItemClusterRenderState state = new ItemClusterRenderState();
            context.set(state.item, ItemDisplayContext.GROUND);
            Capture capture = new Capture();
            state.item.newLayer().setupSpecialModel(capture, null);
            state.seed = seed;
            state.count = ItemClusterRenderState.getRenderedAmount(count);
            ItemEntityRenderer.submitMultipleFromCount(new PoseStack(), null, 0, state,
                    RandomSource.create(0), new AABB(-0.5, -0.5, 0, 0.5, 0.5, depth));
            out.add(capture.offsets);
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(out));
    }
}
