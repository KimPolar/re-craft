import com.google.gson.*;
import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.client.gui.components.AbstractWidget;
import net.minecraft.client.gui.screens.recipebook.OverlayRecipeComponent;
import net.minecraft.client.gui.screens.recipebook.RecipeCollection;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.item.crafting.RecipeBookCategory;
import net.minecraft.world.item.crafting.display.*;

public final class RecipeOverlayOracle {
    @SuppressWarnings("unchecked")
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        Field selected = RecipeCollection.class.getDeclaredField("selected");
        Field craftable = RecipeCollection.class.getDeclaredField("craftable");
        selected.setAccessible(true);
        craftable.setAccessible(true);
        Field x = OverlayRecipeComponent.class.getDeclaredField("x");
        Field y = OverlayRecipeComponent.class.getDeclaredField("y");
        Field buttons = OverlayRecipeComponent.class.getDeclaredField("recipeButtons");
        x.setAccessible(true);
        y.setAccessible(true);
        buttons.setAccessible(true);
        Class<?> buttonClass = Class.forName("net.minecraft.client.gui.screens.recipebook.OverlayRecipeComponent$OverlayRecipeButton");
        Field recipe = buttonClass.getDeclaredField("recipe");
        Field enabled = buttonClass.getDeclaredField("isCraftable");
        recipe.setAccessible(true);
        enabled.setAccessible(true);
        JsonArray cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray result = new JsonArray();
        for (JsonElement element : cases) {
            JsonObject c = element.getAsJsonObject();
            int count = c.get("count").getAsInt();
            int w = c.get("width").getAsInt(), h = c.get("height").getAsInt();
            int anchor = c.get("anchor").getAsInt();
            boolean filtering = c.get("filtering").getAsBoolean();
            int bx = (w - 147) / 2 - (w < 379 ? 0 : 86), by = (h - 166) / 2;
            List<RecipeDisplayEntry> entries = new ArrayList<>();
            var display = new ShapelessCraftingRecipeDisplay(List.of(), SlotDisplay.Empty.INSTANCE, SlotDisplay.Empty.INSTANCE);
            var category = new RecipeBookCategory();
            for (int i = 0; i < count; i++) {
                entries.add(new RecipeDisplayEntry(new RecipeDisplayId(i), display,
                    OptionalInt.empty(), category, Optional.empty()));
            }
            RecipeCollection collection = new RecipeCollection(entries);
            Set<RecipeDisplayId> selectedIds = (Set<RecipeDisplayId>) selected.get(collection);
            Set<RecipeDisplayId> craftableIds = (Set<RecipeDisplayId>) craftable.get(collection);
            for (int i = 0; i < count; i++) {
                RecipeDisplayId id = entries.get(i).id();
                selectedIds.add(id);
                if (i % 2 == 0) craftableIds.add(id);
            }
            OverlayRecipeComponent overlay = new OverlayRecipeComponent(() -> 0, false);
            overlay.init(collection, ContextMap.EMPTY, filtering,
                bx + 11 + anchor % 5 * 25, by + 31 + anchor / 5 * 25,
                bx + 147 / 2, by + 13 + 166 / 2, 25.0f);
            JsonObject out = new JsonObject();
            out.addProperty("x", x.getInt(overlay));
            out.addProperty("y", y.getInt(overlay));
            JsonArray positions = new JsonArray();
            for (Object object : (List<?>) buttons.get(overlay)) {
                AbstractWidget widget = (AbstractWidget) object;
                JsonArray row = new JsonArray();
                row.add(widget.getX());
                row.add(widget.getY());
                row.add(((RecipeDisplayId) recipe.get(object)).index());
                row.add(enabled.getBoolean(object));
                positions.add(row);
            }
            out.add("buttons", positions);
            result.add(out);
        }
        Files.writeString(Path.of(args[1]), result.toString());
    }
}
