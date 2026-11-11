import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.*;
import com.mojang.renderpearl.api.pipeline.RenderPipeline;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.*;
import net.minecraft.client.gui.*;
import net.minecraft.client.gui.components.debug.DebugScreenEntryList;
import net.minecraft.client.multiplayer.MultiPlayerGameMode;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.AttackRange;
import net.minecraft.world.level.GameType;
import net.minecraft.world.phys.*;

public class CrosshairAttackHudOracle {
  static class Capture extends GuiGraphicsExtractor {
    int w, h; JsonArray draws;
    Capture() { super(null, null, 0, 0); }
    @Override public void nextStratum() {}
    @Override public int guiWidth() { return w; }
    @Override public int guiHeight() { return h; }
    @Override public void blitSprite(RenderPipeline p, Identifier id, int x, int y, int w, int h) {
      if (!id.getPath().startsWith("hud/crosshair_attack_indicator")) return;
      var row=new JsonArray(); row.add(id.getPath());
      for(int v:new int[]{x,y,w,h}) row.add(v); draws.add(row);
    }
    @Override public void blitSprite(RenderPipeline p, Identifier id, int sw, int sh, int sx, int sy, int x, int y, int w, int h) {
      if (!id.getPath().startsWith("hud/crosshair_attack_indicator")) return;
      var row=new JsonArray(); row.add(id.getPath());
      for(int v:new int[]{sw,sh,sx,sy,x,y,w,h}) row.add(v); draws.add(row);
    }
  }
  static class Player extends LocalPlayer {
    float charge, delay; boolean alive; ItemStack active;
    Player() { super(null,null,null,null,null,null,false,null,null); }
    @Override public float getAttackStrengthScale(float partial) { return charge; }
    @Override public float getCurrentItemAttackStrengthDelay() { return delay; }
    @Override public ItemStack getActiveItem() { return active; }
    @Override public boolean isAlive() { return alive; }
    @Override public boolean isCreative() { return false; }
  }
  static class Mode extends MultiPlayerGameMode {
    Mode() { super(null,null); }
    @Override public GameType getPlayerMode() { return GameType.SURVIVAL; }
  }
  static class Debug extends DebugScreenEntryList {
    Debug() { super(null,null); }
    @Override public boolean isCurrentlyEnabled(Identifier entry) { return false; }
  }
  static void set(Class<?> type, Object instance, String name, Object value) throws Exception {
    Field field=type.getDeclaredField(name); field.setAccessible(true); field.set(instance,value);
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registries=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    Field field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null);
    var game=(Minecraft)unsafe.allocateInstance(Minecraft.class);
    var player=(Player)unsafe.allocateInstance(Player.class);
    set(Entity.class, player, "position", Vec3.ZERO);
    game.player=player; game.gameMode=(Mode)unsafe.allocateInstance(Mode.class);
    set(Minecraft.class, game, "debugEntries", unsafe.allocateInstance(Debug.class));
    var options=(Options)unsafe.allocateInstance(Options.class);
    var option=(OptionInstance<?>)unsafe.allocateInstance(OptionInstance.class);
    set(OptionInstance.class, option, "value", AttackIndicatorStatus.CROSSHAIR);
    set(Options.class, options, "attackIndicator", option);
    set(Options.class, options, "cameraType", CameraType.FIRST_PERSON);
    set(Minecraft.class, game, "options", options);
    var hud=(Hud)unsafe.allocateInstance(Hud.class); set(Hud.class,hud,"minecraft",game);
    var capture=(Capture)unsafe.allocateInstance(Capture.class);
    var method=Hud.class.getDeclaredMethod("extractCrosshair",GuiGraphicsExtractor.class,DeltaTracker.class);
    method.setAccessible(true);
    var ranged=new ItemStack(Items.STICK);
    ranged.set(DataComponents.ATTACK_RANGE,new AttackRange(2f,4.5f,2f,6.5f,0.125f,0.5f));
    var rows=new JsonArray();
    for(int[] size:new int[][]{{320,240},{321,241}})
    for(float charge:new float[]{0f,0.5f,0.999f,1f})
    for(boolean selected:new boolean[]{false,true})
    for(boolean alive:new boolean[]{false,true})
    for(float delay:new float[]{5f,5.01f})
    for(int reach=0;reach<3;reach++) {
      capture.w=size[0]; capture.h=size[1]; capture.draws=new JsonArray();
      player.charge=charge; player.delay=delay; player.alive=alive;
      player.active=reach==0?ItemStack.EMPTY:ranged;
      game.crosshairPickEntity=selected?player:null;
      game.hitResult=new BlockHitResult(new Vec3(reach==2?8:3,0,0),Direction.UP,BlockPos.ZERO,false);
      method.invoke(hud,capture,null);
      var row=new JsonObject(); row.addProperty("w",size[0]); row.addProperty("h",size[1]);
      row.addProperty("charge",charge); row.addProperty("selected",selected); row.addProperty("alive",alive);
      row.addProperty("delay",delay); row.addProperty("reach",reach); row.add("draws",capture.draws); rows.add(row);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
  }
}
