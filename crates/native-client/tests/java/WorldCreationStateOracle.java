import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.world.Difficulty;
import net.minecraft.world.level.levelgen.WorldOptions;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.client.gui.screens.worldselection.WorldCreationContext;
import net.minecraft.client.gui.screens.worldselection.WorldCreationUiState;
import net.minecraft.client.gui.screens.worldselection.WorldCreationUiState.SelectedGameMode;

public class WorldCreationStateOracle {
    static java.io.PrintWriter output;
    static void field(Object state, String name, Object value) throws Exception {
        Field field = WorldCreationUiState.class.getDeclaredField(name);
        field.setAccessible(true);
        field.set(state, value);
    }
    static void capture(WorldCreationUiState state) {
        output.println("CASE " + state.getDifficulty().name() + " " + state.isAllowCommands() + " " + state.isBonusChest() + " " + state.isGenerateStructures());
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        output = new java.io.PrintWriter(java.nio.file.Files.newBufferedWriter(java.nio.file.Path.of(args[0])));
        var dims = WorldPresets.createNormalWorldDimensions(VanillaRegistries.createWorldLookup());
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)unsafeField.get(null);
        for (var mode : new SelectedGameMode[] {SelectedGameMode.SURVIVAL, SelectedGameMode.HARDCORE, SelectedGameMode.CREATIVE}) {
            for (var difficulty : Difficulty.values()) {
                for (Boolean commands : new Boolean[] {null, false, true}) {
                    for (boolean bonus : new boolean[] {false, true}) {
                        for (boolean structures : new boolean[] {false, true}) {
                            var context = new WorldCreationContext(new WorldOptions(42, structures, bonus), null, dims, null, null, null, null);
                            var state = (WorldCreationUiState)unsafe.allocateInstance(WorldCreationUiState.class);
                            field(state, "settings", context);
                            field(state, "listeners", new ArrayList<>());
                            field(state, "gameMode", mode);
                            field(state, "difficulty", difficulty);
                            field(state, "allowCommands", commands);
                            field(state, "bonusChest", bonus);
                            field(state, "generateStructures", structures);
                            capture(state);
                            state.setGameMode(SelectedGameMode.HARDCORE); capture(state);
                            state.setGameMode(SelectedGameMode.CREATIVE); capture(state);
                        }
                    }
                }
            }
        }
        output.close();
    }
}
