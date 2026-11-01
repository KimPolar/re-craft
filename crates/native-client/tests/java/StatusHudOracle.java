import com.google.gson.*;
import com.mojang.renderpearl.api.pipeline.RenderPipeline;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.*;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.core.Holder;
import net.minecraft.resources.Identifier;
import net.minecraft.world.effect.*;
import net.minecraft.world.entity.LivingEntity;

public class StatusHudOracle {
  static final class Capture extends GuiGraphicsExtractor {
    int width; JsonArray draw;
    Capture() { super(null,null,0,0); }
    @Override public int guiWidth() { return width; }
    @Override public void blitSprite(RenderPipeline p, Identifier id, int x, int y, int w, int h) {
      add(id,x,y,w,h,-1);
    }
    @Override public void blitSprite(RenderPipeline p, Identifier id, int x, int y, int w, int h, int color) {
      add(id,x,y,w,h,color);
    }
    void add(Identifier id,int x,int y,int w,int h,int color) {
      var value=new JsonArray(); value.add(id.getPath()); value.add(x); value.add(y);
      value.add(w); value.add(h); value.add(color); draw.add(value);
    }
  }
  @SuppressWarnings("unchecked")
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)unsafeField.get(null);
    var hud=(Hud)unsafe.allocateInstance(Hud.class);
    var minecraft=(Minecraft)unsafe.allocateInstance(Minecraft.class);
    var gui=(Gui)unsafe.allocateInstance(Gui.class);
    var player=(LocalPlayer)unsafe.allocateInstance(LocalPlayer.class);
    var capture=(Capture)unsafe.allocateInstance(Capture.class);
    Field mcField=Hud.class.getDeclaredField("minecraft"); mcField.setAccessible(true); mcField.set(hud,minecraft);
    Field guiField=Minecraft.class.getDeclaredField("gui"); guiField.setAccessible(true); guiField.set(minecraft,gui);
    Field playerField=Minecraft.class.getDeclaredField("player"); playerField.setAccessible(true); playerField.set(minecraft,player);
    Field demoField=Minecraft.class.getDeclaredField("demo"); demoField.setAccessible(true);
    Field active=LivingEntity.class.getDeclaredField("activeEffects"); active.setAccessible(true);
    var extract=Hud.class.getDeclaredMethod("extractEffects",GuiGraphicsExtractor.class,net.minecraft.client.DeltaTracker.class);
    extract.setAccessible(true);
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var output=new JsonArray();
    for(var element:cases) {
      var c=element.getAsJsonObject(); var effects=new LinkedHashMap<Holder<MobEffect>,MobEffectInstance>();
      capture.width=c.get("width").getAsInt(); capture.draw=new JsonArray(); demoField.setBoolean(minecraft,c.get("demo").getAsBoolean());
      for(var e:c.getAsJsonArray("effects")) {
        var o=e.getAsJsonObject();
        var holder=(Holder<MobEffect>)MobEffects.class.getField(o.get("id").getAsString().toUpperCase(Locale.ROOT)).get(null);
        effects.put(holder,new MobEffectInstance(holder,o.get("duration").getAsInt(),o.get("amplifier").getAsInt(),
            o.get("ambient").getAsBoolean(),true,o.get("icon").getAsBoolean()));
      }
      active.set(player,effects); extract.invoke(hud,capture,null); output.add(capture.draw);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(output));
  }
}
