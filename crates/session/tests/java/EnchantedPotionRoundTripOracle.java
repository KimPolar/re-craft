import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.RegistryOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.item.ItemStack;

/** Decode and re-encode one Rust-generated item using the 26.3 item codec. */
public class EnchantedPotionRoundTripOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var ops = RegistryOps.create(NbtOps.INSTANCE, registries);
        var input = NbtIo.read(Path.of(args[0]));
        var stack = ItemStack.CODEC.parse(ops, input).getOrThrow();
        if (!BuiltInRegistries.ITEM.getKey(stack.getItem()).toString().equals(args[2])) throw new AssertionError("item mismatch");
        var enchantments = stack.get(DataComponents.ENCHANTMENTS);
        if (enchantments == null || enchantments.size() != 1) throw new AssertionError("enchantment missing");
        var enchantment = enchantments.entrySet().iterator().next();
        if (!enchantment.getKey().unwrapKey().orElseThrow().identifier().toString().equals("minecraft:sharpness") || enchantment.getIntValue() < 1 || enchantment.getIntValue() > 5) throw new AssertionError("enchantment mismatch");
        var potion = stack.get(DataComponents.POTION_CONTENTS);
        var potionId = potion == null ? "-" : potion.potion().flatMap(Holder::unwrapKey).map(key -> key.identifier().toString()).orElse("-");
        if (!potionId.equals(args[3])) throw new AssertionError("potion mismatch: " + potionId);
        NbtIo.write((CompoundTag) ItemStack.CODEC.encodeStart(ops, stack).getOrThrow(), Path.of(args[1]));
    }
}
