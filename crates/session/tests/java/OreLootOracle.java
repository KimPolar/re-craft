import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.Holder;
import net.minecraft.util.RandomSource;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.level.storage.loot.*;
import net.minecraft.world.level.storage.loot.providers.number.ints.*;

public class OreLootOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Class<?> oreClass = Class.forName("net.minecraft.world.level.storage.loot.functions.ApplyBonusCount$OreDrops");
        Field instance = oreClass.getDeclaredField("INSTANCE"); instance.setAccessible(true);
        Object ore = instance.get(null);
        Method oreCount = oreClass.getDeclaredMethod("calculateNewCount", RandomSource.class, int.class, int.class); oreCount.setAccessible(true);
        Class<?> uniformClass = Class.forName("net.minecraft.world.level.storage.loot.functions.ApplyBonusCount$UniformBonusCount");
        Constructor<?> uniformCtor = uniformClass.getDeclaredConstructor(int.class); uniformCtor.setAccessible(true);
        Object uniform = uniformCtor.newInstance(1);
        Method uniformCount = uniformClass.getDeclaredMethod("calculateNewCount", RandomSource.class, int.class, int.class); uniformCount.setAccessible(true);
        var ctor = LootContext.class.getDeclaredConstructor(LootParams.class, RandomSource.class, net.minecraft.core.HolderGetter.Provider.class); ctor.setAccessible(true);
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var entry : input) {
            var c = entry.getAsJsonObject();
            String block = c.get("block").getAsString();
            int level = c.get("level").getAsInt();
            boolean silk = c.get("silk").getAsBoolean();
            var random = RandomSource.create(c.get("seed").getAsLong());
            var context = ctor.newInstance(new LootParams(null, ContextMap.builder().build(), Map.of(), 0.0f), random, null);
            int count = 1;
            if (!silk) {
                int min = block.equals("copper_ore") || block.equals("nether_gold_ore") ? 2 : block.equals("lapis_ore") || block.equals("redstone_ore") ? 4 : 1;
                int max = block.equals("copper_ore") || block.equals("redstone_ore") ? 5 : block.equals("nether_gold_ore") ? 6 : block.equals("lapis_ore") ? 9 : 1;
                if (min != max) {
                    var provider = new UniformGenerator(Holder.direct(new ConstantValue(min)), Holder.direct(new ConstantValue(max)));
                    count = provider.getIntUnsafe(context);
                }
                count = (int)(block.equals("redstone_ore") ? uniformCount.invoke(uniform, random, count, level) : oreCount.invoke(ore, random, count, level));
            }
            var row = new JsonArray(); row.add(count); row.add(random.nextInt()); output.add(row);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
