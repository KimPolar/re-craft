import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import net.minecraft.client.gui.components.AbstractWidget;
import net.minecraft.client.gui.components.EditBox;
import net.minecraft.client.gui.Font;
import net.minecraft.client.StringSplitter;
import net.minecraft.client.input.MouseButtonEvent;
import net.minecraft.client.input.MouseButtonInfo;
import net.minecraft.client.input.KeyEvent;

// Compare the real 26.3 EditBox input state without creating a game window.
public final class ChatEditOracle {
    static void set(Object object, Class<?> owner, String name, Object value) throws Exception {
        Field field = owner.getDeclaredField(name);
        field.setAccessible(true);
        field.set(object, value);
    }
    static int get(EditBox box, String name) throws Exception {
        Field field = EditBox.class.getDeclaredField(name);
        field.setAccessible(true);
        return field.getInt(box);
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe) field.get(null);
        String[] texts = {"", "abc  def", " a   b ", "한글  😀-two  끝", "a\u00a0b c", "a\tb c"};
        int[] codes = {1073741904, 1073741903, 8, 127};
        JsonArray rows = new JsonArray();
        for (String text : texts) {
            ArrayList<Integer> positions = new ArrayList<>();
            for (int at = 0; ; at = text.offsetByCodePoints(at, 1)) {
                positions.add(at);
                if (at == text.length()) break;
            }
            for (int cursor : positions) for (int anchor : positions)
            for (int code : codes) for (int modifiers : new int[]{0, 1, 64, 65}) {
                EditBox box = (EditBox) unsafe.allocateInstance(EditBox.class);
                set(box, EditBox.class, "value", text);
                set(box, EditBox.class, "maxLength", 256);
                set(box, EditBox.class, "cursorPos", cursor);
                set(box, EditBox.class, "highlightPos", anchor);
                set(box, EditBox.class, "isEditable", true);
                set(box, AbstractWidget.class, "active", true);
                set(box, AbstractWidget.class, "visible", true);
                set(box, AbstractWidget.class, "focused", true);
                box.keyPressed(new KeyEvent(0, code, modifiers));
                JsonObject row = new JsonObject();
                row.addProperty("before", text);
                row.addProperty("cursor", cursor);
                row.addProperty("anchor", anchor);
                row.addProperty("code", code);
                row.addProperty("modifiers", modifiers);
                row.addProperty("after", box.getValue());
                row.addProperty("afterCursor", get(box, "cursorPos"));
                row.addProperty("afterAnchor", get(box, "highlightPos"));
                rows.add(row);
            }
        }
        Font font = (Font) unsafe.allocateInstance(Font.class);
        set(font, Font.class, "splitter", new StringSplitter((codepoint, style) -> 1.0f));
        for (String text : texts) {
            int count = text.codePointCount(0, text.length());
            for (int x = 0; x <= count + 1; x++) for (boolean twice : new boolean[]{false, true}) {
                EditBox box = (EditBox) unsafe.allocateInstance(EditBox.class);
                set(box, EditBox.class, "value", text);
                set(box, EditBox.class, "font", font);
                set(box, AbstractWidget.class, "width", 256);
                box.onClick(new MouseButtonEvent(x, 0, new MouseButtonInfo(1, 0)), twice);
                JsonObject row = new JsonObject();
                row.addProperty("before", text);
                row.addProperty("clickX", x);
                row.addProperty("double", twice);
                row.addProperty("afterCursor", get(box, "cursorPos"));
                row.addProperty("afterAnchor", get(box, "highlightPos"));
                rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
