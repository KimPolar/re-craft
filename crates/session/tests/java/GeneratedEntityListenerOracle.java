import java.util.function.BiConsumer;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.animal.chicken.Chicken;
import net.minecraft.world.entity.animal.cow.Cow;
import net.minecraft.world.entity.animal.pig.Pig;
import net.minecraft.world.entity.animal.sheep.Sheep;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.monster.warden.Warden;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.EntityBlock;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.block.entity.SculkSensorBlockEntity;
import net.minecraft.world.level.block.entity.SpawnerBlockEntity;

public class GeneratedEntityListenerOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
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
        var chest = new ChestBlockEntity(BlockPos.ZERO, Blocks.CHEST.defaultBlockState());
        var spawner = new SpawnerBlockEntity(BlockPos.ZERO, Blocks.SPAWNER.defaultBlockState());
        var sensor = new SculkSensorBlockEntity(BlockPos.ZERO, Blocks.SCULK_SENSOR.defaultBlockState());
        if (((EntityBlock) Blocks.CHEST).getListener(null, chest) != null ||
            ((EntityBlock) Blocks.SPAWNER).getListener(null, spawner) != null ||
            ((EntityBlock) Blocks.SCULK_SENSOR).getListener(null, sensor) == null) {
            throw new AssertionError("unexpected block-entity listener registration");
        }
    }
}
