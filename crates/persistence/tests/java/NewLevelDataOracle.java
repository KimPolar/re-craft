import java.nio.file.*;
import java.util.*;
import com.mojang.serialization.Dynamic;
import com.mojang.serialization.Lifecycle;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.nbt.*;
import net.minecraft.world.Difficulty;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelSettings;
import net.minecraft.world.level.WorldDataConfiguration;
import net.minecraft.world.level.DataPackConfig;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.storage.LevelData;
import net.minecraft.world.level.storage.PrimaryLevelData;

public class NewLevelDataOracle {
    static Set<String> names(Tag tag) {
        if (tag == null) return null;
        var set = new HashSet<String>();
        for (Tag element : (ListTag)tag) set.add(element.toString());
        return set;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        for (int mode = 0; mode < 4; mode++) for (int difficulty = 0; difficulty < 4; difficulty++)
            for (int flags = 0; flags < 16; flags++) for (int configIndex = 0; configIndex < 2; configIndex++) {
                var config = configIndex == 0 ? WorldDataConfiguration.DEFAULT
                    : new WorldDataConfiguration(new DataPackConfig(List.of("vanilla", "file/custom"), List.of("file/disabled")),
                        FeatureFlags.REGISTRY.subset(FeatureFlags.VANILLA, FeatureFlags.TRADE_REBALANCE, FeatureFlags.REDSTONE_EXPERIMENTS, FeatureFlags.MINECART_IMPROVEMENTS));
                var settings = new LevelSettings("한글 😀", GameType.byId(mode),
                    new LevelSettings.DifficultySettings(Difficulty.byId(difficulty), (flags & 1) != 0, (flags & 2) != 0),
                    (flags & 4) != 0, config);
                var level = new PrimaryLevelData(settings, PrimaryLevelData.SpecialWorldProperty.NONE, Lifecycle.stable());
                var spawn = LevelData.RespawnData.of((mode & 1) == 0 ? Level.OVERWORLD : Level.NETHER, new BlockPos(-123, 70, 456), 45.5f, -12.25f);
                level.setSpawn(spawn); level.setGameTime(-123456789L); level.setInitialized((flags & 8) != 0);
                if ((flags & 2) != 0) { level.setModdedInfo("vanilla", false); level.setModdedInfo("custom", true); level.setModdedInfo("vanilla", false); }
                UUID uuid = (flags & 1) != 0 ? UUID.fromString("00112233-4455-6677-8899-aabbccddeeff") : null;
                var expected = level.createTag(uuid); expected.putLong("LastPlayed", 1234567890123L);
                var root = NbtIo.read(Path.of(args[0], mode + "-" + difficulty + "-" + flags + "-" + configIndex + ".nbt"));
                var actual = (CompoundTag)root.get("Data");
                var loadedConfig = WorldDataConfiguration.CODEC.parse(NbtOps.INSTANCE, actual).getOrThrow();
                var loadedSettings = LevelSettings.parse(new Dynamic<>(NbtOps.INSTANCE, actual), loadedConfig);
                var loadedLevel = PrimaryLevelData.parse(new Dynamic<>(NbtOps.INSTANCE, actual), loadedSettings,
                    PrimaryLevelData.SpecialWorldProperty.NONE, Lifecycle.stable());
                if (loadedLevel.getGameType() != GameType.byId(mode) || loadedLevel.getGameTime() != -123456789L
                    || !loadedLevel.getRespawnData().equals(spawn)) throw new AssertionError("level readback mismatch");
                if (!Objects.equals(names(actual.remove("enabled_features")), names(expected.remove("enabled_features"))))
                    throw new AssertionError("feature flags differ");
                if (!actual.equals(expected)) throw new AssertionError("case " + mode + " " + difficulty + " " + flags + " " + configIndex + "\nactual=" + actual + "\nexpected=" + expected);
            }
    }
}
