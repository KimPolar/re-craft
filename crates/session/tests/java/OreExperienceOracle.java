import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.registries.*;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import net.minecraft.util.valueproviders.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.item.enchantment.*;

public class OreExperienceOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registries = VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var enchantments = registries.lookupOrThrow(Registries.ENCHANTMENT);
        var silk = enchantments.getOrThrow(Enchantments.SILK_TOUCH).value();
        var effects = silk.getEffects(EnchantmentEffectComponents.BLOCK_EXPERIENCE);
        Field range = DropExperienceBlock.class.getDeclaredField("xpRange"); range.setAccessible(true);
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var entry : input) {
            var c = entry.getAsJsonObject();
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(c.get("block").getAsString()));
            IntProvider provider = block instanceof DropExperienceBlock ? (IntProvider) range.get(block) : block instanceof RedStoneOreBlock ? UniformInt.of(1, 5) : null;
            var random = RandomSource.create(c.get("seed").getAsLong());
            var row = new JsonArray();
            if (provider == null) { row.add(-1); row.add(-1); row.add(-1); }
            else {
                int amount = provider.sample(random);
                if (c.get("silk").getAsBoolean()) {
                    float value = amount;
                    for (var effect : effects) {
                        if (effect.requirements().isPresent()) throw new IllegalStateException("unexpected condition");
                        value = effect.effect().process(1, random, value);
                    }
                    amount = (int)value;
                }
                row.add(provider.minInclusive()); row.add(provider.maxInclusive()); row.add(amount);
            }
            row.add(random.nextInt()); output.add(row);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
