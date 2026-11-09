import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.ItemStackTemplate;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.component.UseRemainder;
import net.minecraft.world.level.block.Blocks;

public class BlockUseRemainderOracle {
    static class Player extends VibrationOracle.ProbePlayer {
        boolean creative;
        @Override public boolean hasInfiniteMaterials() { return creative; }
    }

    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var world = (VibrationOracle.World) unsafe.allocateInstance(VibrationOracle.World.class);
        var player = (Player) unsafe.allocateInstance(Player.class);
        var context = (VibrationOracle.ProbePlacement) unsafe.allocateInstance(VibrationOracle.ProbePlacement.class);
        var block = (VibrationOracle.ProbeItem) unsafe.allocateInstance(VibrationOracle.ProbeItem.class);
        Items.STONE.builtInRegistryHolder().bindComponents(DataComponentMap.builder()
            .set(DataComponents.MAX_STACK_SIZE, 64).build());
        world.movementState = Blocks.STONE.defaultBlockState();
        context.world = world;
        context.player = player;
        context.canPlace = true;
        block.success = true;
        var rows = new JsonArray();
        for (boolean creative : new boolean[] { false, true }) {
            for (int count : new int[] { 1, 2, 64 }) {
                player.creative = creative;
                context.stack = new ItemStack(Items.STONE, count);
                context.stack.set(DataComponents.USE_REMAINDER,
                    new UseRemainder(new ItemStackTemplate(Items.DIAMOND)));
                block.place(context);
                var row = new JsonObject();
                row.addProperty("creative", creative);
                row.addProperty("before", count);
                row.addProperty("after", context.stack.getCount());
                row.addProperty("has_remainder", !context.stack.isEmpty()
                    && context.stack.has(DataComponents.USE_REMAINDER));
                rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
