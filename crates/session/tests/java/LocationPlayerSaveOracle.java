import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.world.item.*;
import net.minecraft.world.item.enchantment.*;

public class LocationPlayerSaveOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var ops = lookup.createSerializationContext(NbtOps.INSTANCE);
        var path = Path.of(args[0]);
        var player = NbtIo.readCompressed(path, NbtAccounter.unlimitedHeap());
        if (player.getLongOr("future_data", 0) != 42) throw new AssertionError("player data lost");
        var equipment = player.getCompoundOrEmpty("equipment");
        var bootsTag = equipment.get("feet");
        int expectedDamage = Integer.parseInt(args[1]);
        if (expectedDamage < 0) {
            if (bootsTag != null) throw new AssertionError("broken boots were saved");
        } else {
            if (bootsTag == null) throw new AssertionError("boots missing");
            var boots = ItemStack.CODEC.parse(ops, bootsTag).getOrThrow();
            var soul = lookup.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(Enchantments.SOUL_SPEED);
            if (!boots.is(Items.IRON_BOOTS) || boots.getDamageValue() != expectedDamage
                || EnchantmentHelper.getItemEnchantmentLevel(soul, boots) != 3)
                throw new AssertionError("incorrect saved boots");
            equipment.put("feet", ItemStack.CODEC.encodeStart(ops, boots).getOrThrow());
            player.put("equipment", equipment);
        }
        player.putLong("java_roundtrip", 73);
        NbtIo.writeCompressed(player, path);
    }
}
