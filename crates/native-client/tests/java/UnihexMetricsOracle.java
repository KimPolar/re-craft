import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.client.gui.font.providers.UnihexProvider;

public class UnihexMetricsOracle {
    public static void main(String[] args) throws Exception {
        var json = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
        var provider = json.getAsJsonArray("providers").get(1);
        var definition = UnihexProvider.Definition.CODEC.codec().parse(JsonOps.INSTANCE, provider).getOrThrow();
        var load = UnihexProvider.Definition.class.getDeclaredMethod("loadData", java.io.InputStream.class);
        load.setAccessible(true);
        try (var stream = Files.newInputStream(Path.of(args[1]))) {
            var font = (UnihexProvider) load.invoke(definition, stream);
            int[] codes = font.getSupportedGlyphs().toIntArray();
            java.util.Arrays.sort(codes);
            java.lang.reflect.Method left = null;
            java.lang.reflect.Method right = null;
            for (int code : codes) {
                if (!Character.isValidCodePoint(code) || code >= 0xd800 && code <= 0xdfff) continue;
                var glyph = font.getGlyph(code);
                if (glyph == null) continue;
                if (left == null) {
                    left = glyph.getClass().getDeclaredMethod("left");
                    right = glyph.getClass().getDeclaredMethod("right");
                    left.setAccessible(true);
                    right.setAccessible(true);
                }
                System.out.println(Integer.toHexString(code) + "\t" + left.invoke(glyph) + "\t" + right.invoke(glyph) + "\t" + (int) glyph.info().getAdvance());
            }
        }
    }
}
