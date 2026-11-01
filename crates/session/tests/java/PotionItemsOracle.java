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
        for (var item : java.util.List.of(Items.POTION, Items.TIPPED_ARROW)) {
            for (var potion : java.util.List.of(Potions.WATER, Potions.SWIFTNESS, Potions.POISON, Potions.SLOWNESS, Potions.REGENERATION, Potions.STRENGTH)) {
                var raw = new net.minecraft.nbt.CompoundTag();
                raw.putString("id", BuiltInRegistries.ITEM.getKey(item).toString());
                raw.putInt("count", 1);
                var components = new net.minecraft.nbt.CompoundTag();
                components.putString("minecraft:potion_contents", potion.getRegisteredName());
                raw.put("components", components);
                var decoded = ItemStack.CODEC.parse(ops, raw).getOrThrow();
                var canonical = PotionContents.createItemStack(item, potion);
                int color = decoded.get(DataComponents.POTION_CONTENTS).getColor() & 0xffffff;
                System.out.printf("S %s %s %d %06x %s%n", BuiltInRegistries.ITEM.getKey(item), potion.getRegisteredName(), decoded.getMaxStackSize(), color, ItemStack.isSameItemSameComponents(decoded, canonical));
                var full = new net.minecraft.nbt.CompoundTag();
                full.putString("potion", potion.getRegisteredName());
                full.put("custom_effects", new net.minecraft.nbt.ListTag());
                components.put("minecraft:potion_contents", full);
                var emptyEffects = ItemStack.CODEC.parse(ops, raw).getOrThrow();
                System.out.printf("E %s %s %s%n", BuiltInRegistries.ITEM.getKey(item), potion.getRegisteredName(), ItemStack.isSameItemSameComponents(emptyEffects, canonical));
                full.putInt("custom_color", 0x123456);
                full.putString("custom_name", "test.potion");
                var customized = ItemStack.CODEC.parse(ops, raw).getOrThrow();
                var customizedCopy = ItemStack.CODEC.parse(ops, raw.copy()).getOrThrow();
                System.out.printf("C %s %s %06x %s %s%n", BuiltInRegistries.ITEM.getKey(item), potion.getRegisteredName(), customized.get(DataComponents.POTION_CONTENTS).getColor() & 0xffffff, ItemStack.isSameItemSameComponents(customized, canonical), ItemStack.isSameItemSameComponents(customized, customizedCopy));
            }
        }
        for (var item : java.util.List.of(Items.ARROW, Items.TIPPED_ARROW, Items.MOSS_BLOCK, Items.TUFF, Items.MILK_BUCKET)) {
            var stack = new ItemStack(item);
            System.out.printf("I %s %d%n", BuiltInRegistries.ITEM.getKey(item), stack.getMaxStackSize());
        }
    }
}
