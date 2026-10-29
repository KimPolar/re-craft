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
import net.minecraft.client.renderer.entity.state.ItemEntityRenderState;
import net.minecraft.client.renderer.special.SpecialModelRenderer;
import net.minecraft.client.resources.model.cuboid.ItemTransform;
import net.minecraft.util.RandomSource;
import net.minecraft.world.item.ItemDisplayContext;
import net.minecraft.world.item.ItemStack;
import org.joml.Vector3f;
import org.joml.Vector3fc;

public class GroundItemPoseOracle {
    static class Capture implements SpecialModelRenderer<Object> {
        JsonArray models = new JsonArray();
        public void submit(Object arg, PoseStack pose, SubmitNodeCollector collector,
                int light, int overlay, boolean foil, int outline) {
            JsonArray points = new JsonArray();
            for (float[] point : new float[][]{{0,0,0},{0.5f,0.5f,0.5f},{1,1,1}}) {
                Vector3f transformed = pose.last().pose().transformPosition(new Vector3f(point));
                JsonArray xy = new JsonArray();
                xy.add(transformed.x); xy.add(transformed.y); xy.add(transformed.z);
                points.add(xy);
            }
            models.add(points);
        }
        public void getExtents(Consumer<Vector3fc> consumer) {}
        public Object extractArgument(ItemStack stack) { return null; }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        Field unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var renderer = (ItemEntityRenderer) unsafe.allocateInstance(ItemEntityRenderer.class);
        var randomField = ItemEntityRenderer.class.getDeclaredField("random");
        randomField.setAccessible(true);
        randomField.set(renderer, RandomSource.create(0));
        var context = net.minecraft.client.renderer.item.ItemStackRenderState.class.getDeclaredField("displayContext");
        context.setAccessible(true);
        float[] ages = {0f, 0.5f, 19.75f, 120.25f};
        float[] offsets = {0f, 1.25f, 5.2f};
        int[] counts = {1, 17, 64};
        int[] seeds = {0, 1337};
        JsonArray out = new JsonArray();
        for (int shape = 0; shape < 2; shape++) for (float age : ages)
        for (float bob : offsets) for (int count : counts) for (int seed : seeds) {
            var state = new ItemEntityRenderState();
            context.set(state.item, ItemDisplayContext.GROUND);
            state.ageInTicks = age;
            state.bobOffset = bob;
            state.count = ItemClusterRenderState.getRenderedAmount(count);
            state.seed = seed;
            var capture = new Capture();
            var layer = state.item.newLayer();
            layer.setupSpecialModel(capture, null);
            float z0 = shape == 0 ? 7.5f / 16f : 0f;
            float z1 = shape == 0 ? 8.5f / 16f : 1f;
            layer.setExtents(() -> new Vector3fc[]{
                new Vector3f(0,0,z0), new Vector3f(0,0,z1), new Vector3f(0,1,z0), new Vector3f(0,1,z1),
                new Vector3f(1,0,z0), new Vector3f(1,0,z1), new Vector3f(1,1,z0), new Vector3f(1,1,z1)
            });
            Vector3f rotation = shape == 0 ? new Vector3f(0,0,0) : new Vector3f(15,30,10);
            layer.setItemTransform(new ItemTransform(rotation, new Vector3f(0.05f,0.125f,-0.1f), new Vector3f(0.5f,0.4f,0.6f)));
            renderer.submit(state, new PoseStack(), null, null);
            out.add(capture.models);
        }
        Files.writeString(Path.of(args[0]), new Gson().toJson(out));
    }
}
