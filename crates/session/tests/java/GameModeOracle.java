import com.google.gson.*;
import com.mojang.brigadier.StringReader;
import java.nio.file.*;
import net.minecraft.commands.arguments.GameModeArgument;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.level.GameType;

public class GameModeOracle {
    public static void main(String[] args) throws Exception {
        var rows = new JsonArray();
        for (var name : new String[]{"survival", "creative", "adventure", "spectator", "Creative", "1", "spectatorx", "", "survival:foo"}) {
            var reader = new StringReader(name);
            boolean valid;
            try { GameModeArgument.gameMode().parse(reader); valid = !reader.canRead(); }
            catch (Exception e) { valid = false; }
            var row = new JsonObject(); row.addProperty("input", name); row.addProperty("valid", valid); rows.add(row);
        }
        var abilities = new JsonArray();
        for (var target : GameType.values()) for (boolean initial : new boolean[]{false, true}) {
            var value = new Abilities(); value.flying = initial; target.updatePlayerAbilities(value);
            var row = new JsonObject(); row.addProperty("mode", target.getName()); row.addProperty("initial", initial); row.addProperty("flying", value.flying); abilities.add(row);
        }
        var result = new JsonObject(); result.add("arguments", rows); result.add("abilities", abilities);
        Files.writeString(Path.of(args[0]), result.toString());
    }
}
