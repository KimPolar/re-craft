import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.gamerules.GameRuleMap;
import net.minecraft.world.level.gamerules.GameRules;

public class NewGameRulesOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (int i = 0; i < 2; i++) {
            var data = NbtIo.read(Path.of(args[i])).get("data");
            var rules = GameRuleMap.CODEC.parse(NbtOps.INSTANCE, data).getOrThrow();
            boolean defaults = i == 0;
            if (rules.size() != 14 || rules.get(GameRules.SPAWN_MOBS) != defaults ||
                rules.get(GameRules.ADVANCE_TIME) != defaults ||
                rules.get(GameRules.ADVANCE_WEATHER) != defaults ||
                rules.get(GameRules.WATER_SOURCE_CONVERSION) != defaults ||
                rules.get(GameRules.LAVA_SOURCE_CONVERSION) == defaults ||
                rules.get(GameRules.BLOCK_DROPS) != defaults ||
                rules.get(GameRules.ENTITY_DROPS) != defaults ||
                rules.get(GameRules.NATURAL_HEALTH_REGENERATION) != defaults ||
                rules.get(GameRules.FALL_DAMAGE) != defaults ||
                rules.get(GameRules.KEEP_INVENTORY) == defaults ||
                rules.get(GameRules.IMMEDIATE_RESPAWN) == defaults ||
                rules.get(GameRules.SHOW_DEATH_MESSAGES) != defaults ||
                rules.get(GameRules.RANDOM_TICK_SPEED) != (defaults ? 3 : 17) ||
                rules.get(GameRules.RESPAWN_RADIUS) != (defaults ? 10 : 4)) {
                throw new AssertionError("Java game rules differ: " + rules);
            }
            if (!data.equals(GameRuleMap.CODEC.encodeStart(NbtOps.INSTANCE, rules).getOrThrow())) {
                throw new AssertionError("Java game-rule serialization changed the saved data");
            }
        }
    }
}
