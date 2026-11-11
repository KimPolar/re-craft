import java.util.function.BiConsumer;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.animal.chicken.Chicken;
import net.minecraft.world.entity.animal.cow.Cow;
import net.minecraft.world.entity.animal.pig.Pig;
import net.minecraft.world.entity.animal.sheep.Sheep;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.monster.warden.Warden;

public class GeneratedEntityListenerOracle {
    public static void main(String[] args) throws Exception {
        for (Class<?> type : new Class<?>[] {
            Chicken.class, Cow.class, Pig.class, Sheep.class, ItemEntity.class
        }) {
            Class<?> owner = type.getMethod("updateDynamicGameEventListener", BiConsumer.class)
                .getDeclaringClass();
            if (owner != Entity.class) {
                throw new AssertionError(type.getName() + " overrides listener registration in " + owner);
            }
        }
        if (Warden.class.getMethod("updateDynamicGameEventListener", BiConsumer.class)
            .getDeclaringClass() == Entity.class) {
            throw new AssertionError("warden listener override was not detected");
        }
    }
}
