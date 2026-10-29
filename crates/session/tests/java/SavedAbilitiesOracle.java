import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.storage.TagValueInput;

public class SavedAbilitiesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        var results = new JsonArray();
        for (int index = 0; index < 22; index++) {
            var root = new CompoundTag();
            var saved = new CompoundTag();
            if (index != 0) root.put("abilities", saved);
            switch (index) {
                case 2 -> saved.putFloat("flySpeed", 0.125f);
                case 3 -> saved.putFloat("flySpeed", 0.75f);
                case 4 -> saved.putFloat("flySpeed", -0.1f);
                case 5 -> saved.putByte("flySpeed", (byte) 1);
                case 6 -> saved.putInt("flySpeed", 2);
                case 7 -> saved.putDouble("flySpeed", 0.012345678901);
                case 8 -> saved.putString("flySpeed", "invalid");
                case 9 -> { saved.putFloat("flySpeed", 0.125f); saved.putString("flying", "invalid"); }
                case 10 -> { saved.putFloat("flySpeed", 0.125f); saved.putString("walkSpeed", "invalid"); }
                case 11 -> { saved.putFloat("flySpeed", 0.125f); saved.putInt("mayfly", 2); }
                case 12 -> root.putString("abilities", "invalid");
                case 13 -> saved.putFloat("flySpeed", Float.NaN);
                case 14 -> saved.putFloat("flySpeed", Float.POSITIVE_INFINITY);
                case 15 -> saved.putByte("flying", (byte) 1);
                case 16 -> saved.putByte("flying", (byte) 0);
                case 17 -> saved.putInt("flying", 256);
                case 18 -> saved.putInt("flying", 2);
                case 19 -> saved.putFloat("flying", 0.5f);
                case 20 -> saved.putDouble("flying", -0.25);
                case 21 -> saved.putDouble("flying", Double.NaN);
            }
            var input = TagValueInput.create(ProblemReporter.DISCARDING, lookup, root);
            var abilities = new Abilities();
            input.read("abilities", Abilities.Packed.CODEC).ifPresent(abilities::apply);
            GameType.CREATIVE.updatePlayerAbilities(abilities);
            var row = new JsonObject();
            row.addProperty("index", index);
            row.addProperty("bits", Float.floatToRawIntBits(abilities.getFlyingSpeed()));
            row.addProperty("flying", abilities.flying);
            results.add(row);
        }
        Files.writeString(Path.of(args[0]), results.toString());
    }
}
