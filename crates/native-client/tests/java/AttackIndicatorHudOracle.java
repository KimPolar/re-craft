import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.*;
import com.mojang.renderpearl.api.pipeline.RenderPipeline;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.AttackIndicatorStatus;
import net.minecraft.client.Minecraft;
import net.minecraft.client.OptionInstance;
import net.minecraft.client.Options;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.Hud;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.resources.Identifier;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.HumanoidArm;
import net.minecraft.world.entity.player.Inventory;
import net.minecraft.world.item.ItemStack;

public class AttackIndicatorHudOracle {
  static class Capture extends GuiGraphicsExtractor {
    int w, h; JsonArray draws;
    Capture() { super(null, null, 0, 0); }
    @Override public int guiWidth() { return w; }
    @Override public int guiHeight() { return h; }
    @Override public void blitSprite(RenderPipeline p, Identifier id, int x, int y, int w, int h) {
      if (!id.getPath().startsWith("hud/hotbar_attack_indicator")) return;
      var row = new JsonArray(); row.add(id.getPath());
      for (int v : new int[]{x, y, w, h}) row.add(v);
      draws.add(row);
    }
    @Override public void blitSprite(RenderPipeline p, Identifier id, int sw, int sh, int sx, int sy, int x, int y, int w, int h) {
      if (!id.getPath().startsWith("hud/hotbar_attack_indicator")) return;
      var row = new JsonArray(); row.add(id.getPath());
      for (int v : new int[]{sw, sh, sx, sy, x, y, w, h}) row.add(v);
      draws.add(row);
    }
  }
  static class EmptyInventory extends Inventory {
    EmptyInventory() { super(null, null); }
    @Override public int getSelectedSlot() { return 0; }
    @Override public ItemStack getItem(int index) { return ItemStack.EMPTY; }
  }
  static class Player extends LocalPlayer {
    float charge; HumanoidArm arm; Inventory inventory;
    Player() { super(null, null, null, null, null, null, false, null, null); }
    @Override public Inventory getInventory() { return inventory; }
    @Override public ItemStack getOffhandItem() { return ItemStack.EMPTY; }
    @Override public HumanoidArm getMainArm() { return arm; }
    @Override public float getAttackStrengthScale(float partial) { return charge; }
  }
  static class Game extends Minecraft {
    Entity camera;
    Game() { super(null); }
    @Override public Entity getCameraEntity() { return camera; }
  }
  static void set(Class<?> type, Object instance, String name, Object value) throws Exception {
    Field field = type.getDeclaredField(name); field.setAccessible(true); field.set(instance, value);
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe = (sun.misc.Unsafe)field.get(null);
    var game = (Game)unsafe.allocateInstance(Game.class);
    var player = (Player)unsafe.allocateInstance(Player.class);
    player.inventory = (EmptyInventory)unsafe.allocateInstance(EmptyInventory.class);
    game.camera = player; game.player = player;
    var options = (Options)unsafe.allocateInstance(Options.class);
    var option = (OptionInstance<?>)unsafe.allocateInstance(OptionInstance.class);
    set(OptionInstance.class, option, "value", AttackIndicatorStatus.HOTBAR);
    set(Options.class, options, "attackIndicator", option);
    set(Minecraft.class, game, "options", options);
    var hud = (Hud)unsafe.allocateInstance(Hud.class);
    set(Hud.class, hud, "minecraft", game);
    var capture = (Capture)unsafe.allocateInstance(Capture.class);
    var method = Hud.class.getDeclaredMethod("extractItemHotbar", GuiGraphicsExtractor.class, net.minecraft.client.DeltaTracker.class);
    method.setAccessible(true);
    var rows = new JsonArray();
    for (int[] size : new int[][]{{320,240},{426,240},{641,360}})
    for (boolean left : new boolean[]{false,true})
    for (float charge : new float[]{0f,0.05f,0.5f,0.95f,0.999f,1f}) {
      capture.w=size[0]; capture.h=size[1]; capture.draws=new JsonArray();
      player.arm=left ? HumanoidArm.LEFT : HumanoidArm.RIGHT; player.charge=charge;
      method.invoke(hud, capture, null);
      var row=new JsonObject(); row.addProperty("w",size[0]); row.addProperty("h",size[1]);
      row.addProperty("left",left); row.addProperty("charge",charge); row.add("draws",capture.draws); rows.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
  }
}
