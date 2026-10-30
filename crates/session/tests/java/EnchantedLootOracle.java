import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.registries.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.advancements.predicates.*;
import net.minecraft.util.RandomSource;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.item.*;
import net.minecraft.world.item.enchantment.*;
import net.minecraft.world.level.storage.loot.*;
import net.minecraft.world.level.storage.loot.parameters.LootContextParams;
import net.minecraft.world.level.storage.loot.predicates.BonusLevelTableCondition;

public class EnchantedLootOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registries = VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var enchantments = registries.lookupOrThrow(Registries.ENCHANTMENT);
        var silk = enchantments.getOrThrow(Enchantments.SILK_TOUCH);
        var fortune = enchantments.getOrThrow(Enchantments.FORTUNE);
        var silkPredicate = new EnchantmentPredicate(silk, MinMaxBounds.Ints.atLeast(1));
        var bonus = new BonusLevelTableCondition(fortune, List.of(0.1f, 0.14285715f, 0.25f, 1.0f));
        var ctor = LootContext.class.getDeclaredConstructor(LootParams.class, RandomSource.class, net.minecraft.core.HolderGetter.Provider.class);
        ctor.setAccessible(true);
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var entry : input) {
            var c = entry.getAsJsonObject();
            var stack = new ItemStack(Items.IRON_SHOVEL);
            int silkLevel = c.get("silk").getAsInt(), fortuneLevel = c.get("fortune").getAsInt();
            if (silkLevel > 0) stack.enchant(silk, silkLevel);
            if (fortuneLevel > 0) stack.enchant(fortune, fortuneLevel);
            var random = RandomSource.create(c.get("seed").getAsLong());
            var params = new LootParams(null, ContextMap.builder().set(LootContextParams.TOOL, stack).build(), Map.of(), 0.0f);
            var context = ctor.newInstance(params, random, registries);
            boolean hasSilk = silkPredicate.containedIn(stack.getOrDefault(DataComponents.ENCHANTMENTS, ItemEnchantments.EMPTY));
            String drop = hasSilk ? "gravel" : bonus.test(context) ? "flint" : "gravel";
            var row = new JsonArray(); row.add(drop); row.add(random.nextInt()); output.add(row);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
