import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.*;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.GameType;

public class DamageCooldownOracle {
  static final class Captured extends RuntimeException { }
  static final class Source extends DamageSource {
    boolean bypass;
    Source() { super(Holder.direct(new DamageType("test",0.0f))); }
    @Override public boolean is(TagKey<DamageType> tag) { return bypass && tag == DamageTypeTags.BYPASSES_COOLDOWN; }
  }
  static final class TestPlayer extends Player {
    float captured; boolean called;
    TestPlayer() { super(null,null); }
    @Override public GameType gameMode() { return GameType.SURVIVAL; }
    @Override public boolean isInvulnerableTo(ServerLevel world,DamageSource source) { return false; }
    @Override public boolean isDeadOrDying() { return false; }
    @Override public boolean isSleeping() { return false; }
    @Override public ItemStack getUseItem() { return ItemStack.EMPTY; }
    @Override public float applyItemBlocking(ServerLevel world,DamageSource source,float amount) { return 0.0f; }
    @Override protected void actuallyHurt(ServerLevel world,DamageSource source,float amount) { captured=amount; called=true; throw new Captured(); }
    void setLast(float value) { lastHurt=value; }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
    var abilities=Player.class.getDeclaredField("abilities");
    unsafe.putObject(player,unsafe.objectFieldOffset(abilities),new Abilities());
    var source=new Source(); var out=new JsonArray();
    for(int timer:new int[]{0,1,10,11,19,20}) for(float last:new float[]{0f,1f,2f,10f})
      for(float amount:new float[]{-2f,0f,0.5f,1f,2f,5f,10f,Float.POSITIVE_INFINITY,Float.NEGATIVE_INFINITY,Float.NaN})
      for(boolean bypass:new boolean[]{false,true}) {
        player.damageCooldownTime=timer; player.setLast(last); player.called=false; player.captured=0;
        source.bypass=bypass;
        try { player.hurtServer(null,source,amount); } catch(Captured expected) { }
        var row=new JsonObject(); row.addProperty("timer",timer); row.addProperty("last_bits",Float.floatToRawIntBits(last));
        row.addProperty("amount_bits",Float.floatToRawIntBits(amount)); row.addProperty("bypass",bypass); row.addProperty("accepted",player.called);
        if(player.called) { row.addProperty("damage_bits",Float.floatToRawIntBits(player.captured)); row.addProperty("timer_after",player.damageCooldownTime); }
        out.add(row);
      }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}
