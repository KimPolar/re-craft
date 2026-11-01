import net.minecraft.SharedConstants;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.RegistryOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.alchemy.PotionContents;
import net.minecraft.world.item.alchemy.Potions;

public class PotionItemsOracle {
    public static void main(String[] args) {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var ops = RegistryOps.create(NbtOps.INSTANCE, registries);
        for (var item : java.util.List.of(Items.POTION, Items.TIPPED_ARROW)) {
            for (var potion : java.util.List.of(Potions.POISON, Potions.SLOWNESS, Potions.REGENERATION, Potions.STRENGTH)) {
                var stack = PotionContents.createItemStack(item, potion);
                int color = stack.get(DataComponents.POTION_CONTENTS).getColor() & 0xffffff;
                var encoded = ItemStack.CODEC.encodeStart(ops, stack).getOrThrow();
                System.out.printf("P %s %s %d %06x %s%n", BuiltInRegistries.ITEM.getKey(item), potion.getRegisteredName(), stack.getMaxStackSize(), color, encoded);
            }
        }
        for (var item : java.util.List.of(Items.ARROW, Items.TIPPED_ARROW, Items.MOSS_BLOCK, Items.TUFF, Items.MILK_BUCKET)) {
            var stack = new ItemStack(item);
            System.out.printf("I %s %d%n", BuiltInRegistries.ITEM.getKey(item), stack.getMaxStackSize());
        }
    }
}
