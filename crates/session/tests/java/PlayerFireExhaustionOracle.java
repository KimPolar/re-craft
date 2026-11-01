import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.Holder;
import net.minecraft.resources.Identifier;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.gameevent.GameEvent;

public class PlayerFireExhaustionOracle {
    static class Tracker extends CombatTracker {
        Tracker(LivingEntity e) { super(e); }
        @Override public void recordDamage(DamageSource source,float amount) { }
    }
    static class TestPlayer extends Player {
        float health, absorption, exhaustion; Tracker tracker;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean isInvulnerableTo(ServerLevel world, DamageSource source) { return false; }
        @Override protected float getDamageAfterArmorAbsorb(DamageSource source,float amount) { return amount; }
        @Override protected float getDamageAfterMagicAbsorb(DamageSource source,float amount) { return amount; }
        @Override public float getHealth() { return health; }
        @Override public void setHealth(float value) { health=value; }
        @Override public float getAbsorptionAmount() { return absorption; }
        @Override protected void internalSetAbsorptionAmount(float value) { absorption=value; }
        @Override public double getAttributeValue(Holder<net.minecraft.world.entity.ai.attributes.Attribute> attribute) { return 100.0; }
        @Override public CombatTracker getCombatTracker() { return tracker; }
        @Override public void causeFoodExhaustion(float value) { exhaustion=Math.min(exhaustion+value,40.0f); }
        @Override public void awardStat(Identifier id,int amount) { }
        @Override public void gameEvent(Holder<GameEvent> event) { }
        void apply(DamageSource source,float amount) { actuallyHurt(null,source,amount); }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.tracker=new Tracker(player);
        var rows=new JsonArray();
        for(float absorption:new float[]{0f,2f,4f,8f}) for(float exhaustion:new float[]{0f,0.1f,0.75f}) for(float amount:new float[]{1f,4f}) {
            player.health=20f; player.absorption=absorption; player.exhaustion=0f;
            var source=new DamageSource(Holder.direct(new DamageType("test",exhaustion)));
            player.apply(source,amount);
            var row=new JsonObject(); row.addProperty("initial_absorption",absorption); row.addProperty("source_exhaustion",exhaustion); row.addProperty("damage",amount);
            row.addProperty("health_bits",Float.floatToRawIntBits(player.health)); row.addProperty("absorption_bits",Float.floatToRawIntBits(player.absorption)); row.addProperty("exhaustion_bits",Float.floatToRawIntBits(player.exhaustion)); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
