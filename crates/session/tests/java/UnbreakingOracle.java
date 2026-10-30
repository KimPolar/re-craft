import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.item.enchantment.LevelBasedValue;
import net.minecraft.world.item.enchantment.effects.RemoveBinomial;

public class UnbreakingOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var entry : input) {
            var c = entry.getAsJsonObject();
            boolean armor = c.get("armor").getAsBoolean();
            var chance = new LevelBasedValue.Fraction(
                new LevelBasedValue.Linear(armor ? 2.0f : 1.0f, armor ? 2.0f : 1.0f),
                new LevelBasedValue.Linear(armor ? 10.0f : 2.0f, armor ? 5.0f : 1.0f));
            var random = RandomSource.create(c.get("seed").getAsLong());
            var effect = new RemoveBinomial(chance);
            var row = new JsonArray();
            row.add(Float.floatToRawIntBits(effect.process(c.get("level").getAsInt(), random, c.get("amount").getAsFloat())));
            row.add(random.nextInt());
            output.add(row);
        }
        Files.writeString(Path.of(args[1]), output.toString());
    }
}
