import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.client.gui.*;
import net.minecraft.client.gui.screens.inventory.*;
import net.minecraft.network.chat.*;
import net.minecraft.util.FormattedCharSequence;
import net.minecraft.world.inventory.tooltip.TooltipComponent;

public class EffectTooltipOracle {
  static class TestFont extends Font {
    TestFont() { super(null); }
    @Override public int width(FormattedText text) { return text.getString().length() * 6; }
    @Override public FormattedText substrByWidth(FormattedText text,int width) {
      var source=text.getString(); return Component.literal(source.substring(0,Math.min(source.length(),Math.max(0,width/6))));
    }
  }
  static class TestScreen extends InventoryScreen {
    Font testFont;
    TestScreen() { super(null); }
    @Override public Font getFont() { return testFont; }
  }
  static class Capture extends GuiGraphicsExtractor {
    boolean tooltip;
    Capture() { super(null,null,0,0); }
    @Override public void text(Font f, FormattedCharSequence text, int x,int y,int color) {}
    @Override public void text(Font f, Component text, int x,int y,int color) {}
    @Override public void setTooltipForNextFrame(Font f, List<Component> lines, Optional<TooltipComponent> extra, int x,int y) {
      tooltip=true;
    }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null);
    var font=(TestFont)unsafe.allocateInstance(TestFont.class);
    var screen=(TestScreen)unsafe.allocateInstance(TestScreen.class); screen.testFont=font;
    var effects=(EffectsInInventory)unsafe.allocateInstance(EffectsInInventory.class);
    var screenField=EffectsInInventory.class.getDeclaredField("screen"); screenField.setAccessible(true); screenField.set(effects,screen);
    var capture=(Capture)unsafe.allocateInstance(Capture.class);
    var method=EffectsInInventory.class.getDeclaredMethod("extractText",GuiGraphicsExtractor.class,Component.class,Component.class,Font.class,int.class,int.class,int.class,int.class,int.class,int.class);
    method.setAccessible(true);
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray(); var result=new JsonArray();
    for(var element:cases) {
      var c=element.getAsJsonObject(); capture.tooltip=false;
      method.invoke(effects,capture,Component.literal("ABCDEFGHIJKLMNOPQRSTUVWXYZ"),Component.literal("01:00"),font,
        c.get("x").getAsInt(),c.get("y").getAsInt(),c.get("width").getAsInt(),c.get("spacing").getAsInt(),c.get("mouse_x").getAsInt(),c.get("mouse_y").getAsInt());
      result.add(capture.tooltip);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(result));
  }
}
