import com.google.gson.*;
import com.mojang.renderpearl.api.pipeline.RenderPipeline;
import java.nio.file.*;
import java.util.List;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.Font;
import net.minecraft.client.gui.screens.inventory.tooltip.ClientTooltipComponent;
import net.minecraft.client.gui.screens.inventory.tooltip.DefaultTooltipPositioner;
import net.minecraft.client.gui.screens.inventory.tooltip.TooltipRenderUtil;
import net.minecraft.resources.Identifier;

public class TooltipGeometryOracle {
  static class Capture extends GuiGraphicsExtractor {
    JsonArray draws, text;
    int screenWidth, screenHeight;
    Capture() { super(null,null,0,0); }
    @Override public int guiWidth() { return screenWidth; }
    @Override public int guiHeight() { return screenHeight; }
    @Override public void blitSprite(RenderPipeline pipeline, Identifier id, int x,int y,int w,int h) {
      var draw=new JsonArray(); draw.add(id.getPath()); draw.add(x); draw.add(y); draw.add(w); draw.add(h); draws.add(draw);
    }
  }
  static class Line implements ClientTooltipComponent {
    int width;
    Line(int width) { this.width=width; }
    @Override public int getWidth(Font ignored) { return width; }
    @Override public int getHeight(Font ignored) { return 10; }
    @Override public void extractText(GuiGraphicsExtractor graphics, Font ignored, int x, int y) {
      var point=new JsonArray(); point.add(x); point.add(y); ((Capture)graphics).text.add(point);
    }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null); var capture=(Capture)unsafe.allocateInstance(Capture.class);
    var pose=GuiGraphicsExtractor.class.getDeclaredField("pose"); pose.setAccessible(true);
    pose.set(capture,new org.joml.Matrix3x2fStack(16));
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var result=new JsonArray();
    for (var element:cases) {
      var c=element.getAsJsonObject();
      int sw=c.get("screen_width").getAsInt(), sh=c.get("screen_height").getAsInt();
      int mx=c.get("mouse_x").getAsInt(), my=c.get("mouse_y").getAsInt();
      int w=c.get("width").getAsInt(), h=c.get("height").getAsInt();
      capture.screenWidth=sw; capture.screenHeight=sh; capture.draws=new JsonArray(); capture.text=new JsonArray();
      capture.tooltip(null,List.of(new Line(w),new Line(w/2)),mx,my,DefaultTooltipPositioner.INSTANCE,null,false);
      var row=new JsonObject(); row.add("text",capture.text); row.add("draws",capture.draws);
      result.add(row);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(result));
  }
}
