import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.client.renderer.texture.Stitcher;
import net.minecraft.client.renderer.texture.StitcherException;
import net.minecraft.resources.Identifier;

/** Calls the user's installed 26.3 Stitcher with synthetic sprite dimensions. */
public final class StitcherOracle {
    record Entry(Identifier name, int width, int height) implements Stitcher.Entry {}

    public static void main(String[] args) throws Exception {
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var result = new JsonArray();
        for (var element : cases) {
            var input = element.getAsJsonObject();
            var stitcher = new Stitcher<Entry>(input.get("max_width").getAsInt(), input.get("max_height").getAsInt(),
                    input.get("mip").getAsInt(), input.get("anisotropy").getAsInt());
            for (var sprite : input.getAsJsonArray("sprites")) {
                var value = sprite.getAsJsonObject();
                stitcher.registerSprite(new Entry(Identifier.parse(value.get("id").getAsString()),
                        value.get("width").getAsInt(), value.get("height").getAsInt()));
            }
            var output = new JsonObject();
            try {
                stitcher.stitch();
                output.addProperty("width", stitcher.getWidth());
                output.addProperty("height", stitcher.getHeight());
                var placements = new JsonObject();
                stitcher.gatherSprites((entry, x, y, padding) -> {
                    var position = new JsonArray();
                    position.add(x); position.add(y); position.add(padding);
                    placements.add(entry.name().toString(), position);
                });
                output.add("placements", placements);
            } catch (StitcherException error) {
                output.addProperty("error", true);
            }
            result.add(output);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(result));
    }
}
