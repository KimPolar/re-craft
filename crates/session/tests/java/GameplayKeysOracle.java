import com.mojang.blaze3d.platform.InputConstants;
import net.minecraft.client.KeyMapping;

public class GameplayKeysOracle {
    private static final KeyMapping[] HOTBAR = new KeyMapping[9];
    private static final KeyMapping SWAP = new KeyMapping("test.swap", 50, KeyMapping.Category.GAMEPLAY);
    private static final KeyMapping DROP = new KeyMapping("test.drop", 51, KeyMapping.Category.GAMEPLAY);

    private static void click(int key) {
        KeyMapping.click(InputConstants.Type.KEYBOARD.getOrCreate(key));
    }

    private static void tick() {
        StringBuilder result = new StringBuilder();
        for (int slot = 0; slot < HOTBAR.length; slot++) {
            if (HOTBAR[slot].consumeClick()) result.append("H").append(slot).append(",");
        }
        while (SWAP.consumeClick()) result.append("S,");
        while (DROP.consumeClick()) result.append("D,");
        System.out.println(result);
    }

    public static void main(String[] args) {
        for (int slot = 0; slot < HOTBAR.length; slot++) {
            HOTBAR[slot] = new KeyMapping("test.hotbar." + slot, slot + 10, KeyMapping.Category.INVENTORY);
        }
        click(51); click(50); click(11); click(10); click(10);
        tick();
        tick();
        click(14); click(51);
        KeyMapping.releaseAll();
        click(51);
        tick();
    }
}
