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
import com.mojang.brigadier.CommandDispatcher;
import net.minecraft.commands.CommandSourceStack;
import net.minecraft.server.commands.SeedCommand;
import net.minecraft.server.permissions.PermissionSet;
import net.minecraft.network.chat.Component;
import net.minecraft.network.chat.ComponentUtils;
import net.minecraft.network.chat.ClickEvent;
import net.minecraft.network.chat.contents.TranslatableContents;
import net.minecraft.commands.arguments.TimeArgument;
import com.mojang.brigadier.StringReader;
import net.minecraft.world.clock.ClockTimeMarker;
import java.util.Optional;
import net.minecraft.world.clock.ServerClockManager;
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
        CommandSourceStack source = (CommandSourceStack) unsafe.allocateInstance(CommandSourceStack.class);
        for (boolean dedicated : new boolean[]{false, true}) {
            CommandDispatcher<CommandSourceStack> dispatcher = new CommandDispatcher<>();
            SeedCommand.register(dispatcher, dedicated);
            for (boolean allowed : new boolean[]{false, true}) {
                set(source, CommandSourceStack.class, "permissions", allowed ? PermissionSet.ALL_PERMISSIONS : PermissionSet.NO_PERMISSIONS);
                if (dispatcher.getRoot().getChild("seed").canUse(source) != (!dedicated || allowed))
                    throw new AssertionError("seed permissions");
            }
        }
        String seed = "-9223372036854775808";
        Component wrapped = ComponentUtils.copyOnClickText(seed);
        TranslatableContents bracket = (TranslatableContents) wrapped.getContents();
        Component digit = (Component) bracket.getArgs()[0];
        if (!bracket.getKey().equals("chat.square_brackets") ||
            digit.getStyle().getColor().getValue() != 0x55ff55 ||
            !(digit.getStyle().getClickEvent() instanceof ClickEvent.CopyToClipboard copy) ||
            !copy.value().equals(seed) || !seed.equals(digit.getStyle().getInsertion()) ||
            digit.getStyle().getHoverEvent() == null || digit.getStyle().isUnderlined())
            throw new AssertionError("seed component style");
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
        ArrayList<String> times = new ArrayList<>();
        for (int n = -200; n <= 200; n++) times.add(Float.toString(n / 10.0f));
        for (String special : new String[]{"", "-", ".", "+1", "1e3", "-0.5", "-0.00001", "2147483647", "2147483648", "999999999999999999999999999999999999999999", "1.2.3", "--1"}) times.add(special);
        for (String time : times) for (String unit : new String[]{"", "t", "s", "d", "x"}) for (int minimum : new int[]{0, Integer.MIN_VALUE}) {
            String input = time + unit;
            JsonObject row = new JsonObject();
            row.addProperty("timeArgument", input);
            row.addProperty("minimum", minimum);
            try {
                StringReader reader = new StringReader(input);
                int ticks = TimeArgument.time(minimum).parse(reader);
                if (reader.canRead()) throw new IllegalArgumentException("trailing characters");
                row.addProperty("ticks", ticks);
            } catch (Exception ignored) {
                row.add("ticks", com.google.gson.JsonNull.INSTANCE);
            }
            rows.add(row);
        }
        for (int period : new int[]{0, 1, 48, 24000, Integer.MAX_VALUE})
        for (int marker : new int[]{0, 1, 5, 47, 1000, 18000, Integer.MAX_VALUE})
        for (long current : new long[]{Long.MIN_VALUE, -24001, -1, 0, 1, 5, 47, 48, 1000, 23999, 24000, Long.MAX_VALUE}) {
            ClockTimeMarker definition = new ClockTimeMarker(null, marker, period == 0 ? Optional.empty() : Optional.of(period), true);
            JsonObject row = new JsonObject();
            row.addProperty("markerTicks", marker);
            row.addProperty("markerPeriod", period);
            row.addProperty("markerCurrent", current);
            if (definition.occursAt(current)) row.add("markerTarget", com.google.gson.JsonNull.INSTANCE);
            else row.addProperty("markerTarget", definition.resolveTimeToMoveTo(current));
            rows.add(row);
        }
        Class<?> instanceType = ServerClockManager.ServerClockInstance.class;
        var addTicks = ServerClockManager.class.getDeclaredMethod("lambda$addTicks$0", int.class, instanceType);
        var setTicks = ServerClockManager.class.getDeclaredMethod("lambda$setTotalTicks$0", long.class, instanceType);
        addTicks.setAccessible(true);
        setTicks.setAccessible(true);
        for (long initial : new long[]{Long.MIN_VALUE, -1, 0, 1, 1000, Long.MAX_VALUE})
        for (int amount : new int[]{Integer.MIN_VALUE, -1, 0, 1, 1000, Integer.MAX_VALUE})
        for (boolean add : new boolean[]{false, true}) {
            var clock = new ServerClockManager.ServerClockInstance();
            set(clock, instanceType, "totalTicks", initial);
            set(clock, instanceType, "partialTick", 0.75f);
            if (add) addTicks.invoke(null, amount, clock);
            else if (initial != amount) setTicks.invoke(null, (long) amount, clock);
            Field total = instanceType.getDeclaredField("totalTicks");
            Field partial = instanceType.getDeclaredField("partialTick");
            total.setAccessible(true); partial.setAccessible(true);
            JsonObject row = new JsonObject();
            row.addProperty("clockInitial", initial);
            row.addProperty("clockAmount", amount);
            row.addProperty("clockAdd", add);
            row.addProperty("clockTotal", total.getLong(clock));
            row.addProperty("clockPartial", partial.getFloat(clock));
            rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
