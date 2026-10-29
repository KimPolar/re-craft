import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.Registries;
import net.minecraft.tags.DamageTypeTags;
import net.minecraft.tags.TagKey;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.flag.FeatureFlags;

public class OrbLavaOracle {
  static class World extends ServerLevel {
    GameRules rules;
    World() { super(null,null,null,null,null,null,false,0L,null,false); }
    @Override public GameRules getGameRules() { return rules; }
  }
  static class Orb extends ExperienceOrb {
    World world; DamageSources sources; boolean removed; boolean inLava;
    Orb() { super(null,0,0,0,7); }
    @Override public Level level() { return world; }
    @Override public EntityType<?> getType() { return EntityTypes.EXPERIENCE_ORB; }
    @Override public DamageSources damageSources() { return sources; }
    @Override public void gameEvent(Holder<GameEvent> event, Entity source) { }
    @Override public void clearFreeze() { }
    @Override public void remove(Entity.RemovalReason reason) { removed=true; }
    @Override public boolean isSilent() { return true; }
    @Override protected void computeSpeed() { }
    @Override protected void handlePortal() { }
    @Override public boolean canSpawnSprintParticle() { return false; }
    @Override public boolean isEyeInFluid(TagKey<Fluid> tag) { return false; }
    @Override protected boolean updateFluidInteraction() { return false; }
    @Override public void updateSwimming() { }
    @Override public boolean isInLava() { return inLava; }
    @Override public void checkBelowWorld() { }
    @Override public void setSharedFlagOnFire(boolean value) { }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registry=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    var lookup=registry.lookupOrThrow(Registries.DAMAGE_TYPE);
    var lava=lookup.getOrThrow(DamageTypes.LAVA); var fire=lookup.getOrThrow(DamageTypes.ON_FIRE);
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
    bind.invoke(lava,List.of(DamageTypeTags.IS_FIRE)); bind.invoke(fire,List.of(DamageTypeTags.IS_FIRE));
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var sources=(DamageSources)unsafe.allocateInstance(DamageSources.class);
    var lavaField=DamageSources.class.getDeclaredField("lava"); lavaField.setAccessible(true); lavaField.set(sources,new DamageSource(lava));
    var fireField=DamageSources.class.getDeclaredField("onFire"); fireField.setAccessible(true); fireField.set(sources,new DamageSource(fire));
    var world=(World)unsafe.allocateInstance(World.class); world.rules=new GameRules(FeatureFlags.DEFAULT_FLAGS);
    var health=ExperienceOrb.class.getDeclaredField("health"); health.setAccessible(true);
    var invuln=Entity.class.getDeclaredField("permanentlyInvulnerable"); invuln.setAccessible(true);
    var out=new JsonArray();
    for(boolean immune:new boolean[]{false,true}) {
      var orb=(Orb)unsafe.allocateInstance(Orb.class); orb.world=world; orb.sources=sources;
      health.setInt(orb,5); invuln.setBoolean(orb,immune);
      if(orb.fireImmune()) throw new AssertionError("orb is fire immune");
      var rows=new JsonArray();
      for(int tick=0;tick<2;tick++) {
        orb.lavaIgnite(); orb.lavaHurt();
        var row=new JsonObject(); row.addProperty("health",health.getInt(orb));
        row.addProperty("fire",orb.getRemainingFireTicks()); row.addProperty("removed",orb.removed); rows.add(row);
        if(orb.removed) break;
      }
      out.add(rows);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
    var fireOut=new JsonArray();
    for(int mode=0;mode<4;mode++) {
      var orb=(Orb)unsafe.allocateInstance(Orb.class); orb.world=world; orb.sources=sources;
      health.setInt(orb,5); invuln.setBoolean(orb,mode%2==1); orb.inLava=mode>=2; orb.setRemainingFireTicks(101);
      var rows=new JsonArray();
      for(int tick=0;tick<102;tick++) {
        orb.baseTick();
        var row=new JsonObject(); row.addProperty("health",health.getInt(orb));
        row.addProperty("fire",orb.getRemainingFireTicks()); row.addProperty("removed",orb.removed); rows.add(row);
        if(orb.removed) break;
      }
      fireOut.add(rows);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(fireOut));
  }
}
