import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.BlockPos;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.block.Blocks;

public class ItemFallDistanceOracle {
    static class Item extends ItemEntity {
        boolean water;
        Item() { super(null, 0, 0, 0, ItemStack.EMPTY); }
        @Override public boolean isInWater() { return water; }
        void apply(double movement, boolean grounded) {
            checkFallDamage(movement, grounded, Blocks.AIR.defaultBlockState(), BlockPos.ZERO);
        }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        var f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) f.get(null);
        var item = (Item) unsafe.allocateInstance(Item.class);
        var rows = new JsonArray();
        for (boolean water : new boolean[]{false, true})
            for (double initial : new double[]{0, 0.5, 5, 100000000})
                for (double movement : new double[]{-3.141592653589793, -0.2, -1e-12, 0, 0.2}) {
                    item.water = water; item.fallDistance = initial;
                    item.apply(movement, false);
                    var row = new JsonObject();
                    row.addProperty("water", water); row.addProperty("initial", initial);
                    row.addProperty("movement", movement);
                    row.addProperty("fall_bits", Double.doubleToRawLongBits(item.fallDistance));
                    rows.add(row);
                }
        item.water = false; item.fallDistance = 0; item.apply(0.2, true);
        if (item.fallDistance != 0) throw new AssertionError("ground reset");
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
