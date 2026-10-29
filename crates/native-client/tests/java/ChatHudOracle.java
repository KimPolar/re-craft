import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.function.Consumer;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.client.Minecraft;
import net.minecraft.client.OptionInstance;
import net.minecraft.client.Options;
import net.minecraft.client.gui.Font;
import net.minecraft.client.gui.Gui;
import net.minecraft.client.gui.components.ChatComponent;
import net.minecraft.client.gui.components.ChatComponent.ChatGraphicsAccess;
import net.minecraft.client.gui.components.ChatComponent.DisplayMode;
import net.minecraft.client.multiplayer.chat.ChatListener;
import net.minecraft.client.multiplayer.chat.GuiMessage;
import net.minecraft.client.multiplayer.chat.GuiMessageSource;
import net.minecraft.client.multiplayer.chat.GuiMessageTag;
import net.minecraft.network.chat.Component;
import net.minecraft.util.FormattedCharSequence;
import org.joml.Matrix3x2f;

// Exercises Minecraft's actual chat layout code without opening a client window.
public class ChatHudOracle {
    static sun.misc.Unsafe unsafe;
    static void set(Object object, String name, Object value) throws Exception {
        Field field = object.getClass().getDeclaredField(name);
        field.setAccessible(true);
        field.set(object, value);
    }
    static OptionInstance<?> option(double value) throws Exception {
        OptionInstance<?> instance = (OptionInstance<?>) unsafe.allocateInstance(OptionInstance.class);
        set(instance, "value", value);
        return instance;
    }
    static JsonArray ints(int... values) {
        JsonArray result = new JsonArray();
        for (int value : values) result.add(value);
        return result;
    }
    static class Recorder implements ChatGraphicsAccess {
        JsonArray background;
        JsonArray tag;
        int messageY;
        int opacityBits;
        boolean hasMessage;
        public void updatePose(Consumer<Matrix3x2f> update) { update.accept(new Matrix3x2f()); }
        public void fill(int x1, int y1, int x2, int y2, int color) {
            background = ints(x1, y1, x2, y2, color);
        }
        public boolean handleMessage(int y, float opacity, FormattedCharSequence text) {
            messageY = y; opacityBits = Float.floatToIntBits(opacity); hasMessage = true;
            return false;
        }
        public void handleTag(int x1, int y1, int x2, int y2, float opacity, GuiMessageTag value) {
            tag = ints(x1, y1, x2, y2, Float.floatToIntBits(opacity), value.indicatorColor());
        }
        public void handleTagIcon(int x, int y, boolean hovered, GuiMessageTag tag, GuiMessageTag.Icon icon) {
            throw new AssertionError("single-player system messages have no icon");
        }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        unsafe = (sun.misc.Unsafe)field.get(null);
        Minecraft minecraft = (Minecraft)unsafe.allocateInstance(Minecraft.class);
        Options options = (Options)unsafe.allocateInstance(Options.class);
        Gui gui = (Gui)unsafe.allocateInstance(Gui.class);
        set(minecraft, "options", options);
        set(minecraft, "gui", gui);
        set(minecraft, "font", unsafe.allocateInstance(Font.class));
        set(gui, "chatListener", new ChatListener(minecraft));
        ChatComponent chat = (ChatComponent)unsafe.allocateInstance(ChatComponent.class);
        set(chat, "minecraft", minecraft);
        GuiMessage message = new GuiMessage(0, Component.empty(), null, GuiMessageSource.SYSTEM_SERVER, GuiMessageTag.systemSinglePlayer());
        set(chat, "trimmedMessages", List.of(new GuiMessage.Line(message, FormattedCharSequence.EMPTY, true)));
        set(options, "chatHeightUnfocused", option(0.3125));
        Method draw = ChatComponent.class.getDeclaredMethod("extractRenderState", ChatGraphicsAccess.class, int.class, int.class, DisplayMode.class);
        draw.setAccessible(true);
        JsonArray rows = new JsonArray();
        for (int height : new int[]{240, 360, 600})
        for (int age : new int[]{0, 100, 180, 181, 190, 199, 200})
        for (double scale : new double[]{0.5, 1.0})
        for (double width : new double[]{0.25, 1.0})
        for (double opacity : new double[]{0.0, 0.5, 1.0})
        for (double background : new double[]{0.2, 0.5, 1.0})
        for (double spacing : new double[]{0.0, 0.5, 1.0}) {
            set(options, "chatScale", option(scale));
            set(options, "chatWidth", option(width));
            set(options, "chatOpacity", option(opacity));
            set(options, "textBackgroundOpacity", option(background));
            set(options, "chatLineSpacing", option(spacing));
            Recorder recorder = new Recorder();
            draw.invoke(chat, recorder, height, age, DisplayMode.BACKGROUND);
            JsonObject row = new JsonObject();
            row.addProperty("height", height); row.addProperty("age", age);
            row.addProperty("scale", scale); row.addProperty("width", width);
            row.addProperty("opacity", opacity); row.addProperty("backgroundOpacity", background);
            row.addProperty("spacing", spacing); row.addProperty("visible", recorder.hasMessage);
            if (recorder.hasMessage) {
                row.add("background", recorder.background); row.add("tag", recorder.tag);
                row.addProperty("messageY", recorder.messageY); row.addProperty("opacityBits", recorder.opacityBits);
            }
            rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
