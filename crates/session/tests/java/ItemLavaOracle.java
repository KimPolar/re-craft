import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.tags.DamageTypeTags;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.*;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.flag.FeatureFlags;

public class ItemLavaOracle {
  static class World extends ServerLevel {
    GameRules rules;
    World() { super(null,null,null,null,null,null,false,0L,null,false); }
    @Override public GameRules getGameRules() { return rules; }
  }
  static class Item extends ItemEntity {
    World world; ItemStack stack; DamageSources sources; boolean removed;
    Item() { super(null,0,0,0,ItemStack.EMPTY); }
    @Override public Level level() { return world; }
    @Override public EntityType<?> getType() { return EntityTypes.ITEM; }
    @Override public ItemStack getItem() { return stack; }
    @Override public DamageSources damageSources() { return sources; }
    @Override public void gameEvent(Holder<GameEvent> event, Entity source) { }
    @Override public void clearFreeze() { }
    @Override public void remove(Entity.RemovalReason reason) { removed=true; }
    @Override public boolean isSilent() { return true; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registry=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registry)) pending.apply();
    var lookup=registry.lookupOrThrow(Registries.DAMAGE_TYPE);
    var lava=lookup.getOrThrow(DamageTypes.LAVA); var inFire=lookup.getOrThrow(DamageTypes.IN_FIRE);
    var onFire=lookup.getOrThrow(DamageTypes.ON_FIRE);
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
    bind.invoke(lava,List.of(DamageTypeTags.IS_FIRE)); bind.invoke(inFire,List.of(DamageTypeTags.IS_FIRE));
    bind.invoke(onFire,List.of(DamageTypeTags.IS_FIRE));
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var sources=(DamageSources)unsafe.allocateInstance(DamageSources.class);
    var lavaField=DamageSources.class.getDeclaredField("lava"); lavaField.setAccessible(true); lavaField.set(sources,new DamageSource(lava));
    var fireField=DamageSources.class.getDeclaredField("inFire"); fireField.setAccessible(true); fireField.set(sources,new DamageSource(inFire));
    var onFireField=DamageSources.class.getDeclaredField("onFire"); onFireField.setAccessible(true); onFireField.set(sources,new DamageSource(onFire));
    var world=(World)unsafe.allocateInstance(World.class); world.rules=new GameRules(FeatureFlags.DEFAULT_FLAGS);
    var health=ItemEntity.class.getDeclaredField("health"); health.setAccessible(true);
    var invuln=Entity.class.getDeclaredField("permanentlyInvulnerable"); invuln.setAccessible(true);
    var out=new JsonArray();
    for(int mode=0;mode<3;mode++) {
      var item=(Item)unsafe.allocateInstance(Item.class); item.world=world; item.sources=sources;
      item.stack=new ItemStack(mode==1 ? Items.NETHERITE_INGOT : Items.STONE);
      health.setInt(item,5); invuln.setBoolean(item,mode==2);
      var rows=new JsonArray();
      for(int tick=0;tick<2;tick++) {
        item.lavaIgnite(); item.lavaHurt();
        var row=new JsonObject(); row.addProperty("health",health.getInt(item));
        row.addProperty("fire",item.getRemainingFireTicks()); row.addProperty("removed",item.removed); rows.add(row);
        if(item.removed) break;
      }
      out.add(rows);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
    var fireOut=new JsonArray();
    for(int mode=0;mode<3;mode++) {
      var item=(Item)unsafe.allocateInstance(Item.class); item.world=world; item.sources=sources;
      item.stack=new ItemStack(mode==1 ? Items.NETHERITE_INGOT : Items.STONE);
      health.setInt(item,5); invuln.setBoolean(item,mode==2); item.setRemainingFireTicks(41);
      var rows=new JsonArray();
      for(int tick=0;tick<22;tick++) {
        if(item.getRemainingFireTicks()>0) {
          if(item.fireImmune()) item.clearFire();
          else {
            if(item.getRemainingFireTicks()%20==0) item.hurtServer(world,sources.onFire(),1.0f);
            item.setRemainingFireTicks(item.getRemainingFireTicks()-1);
          }
        }
        var row=new JsonObject(); row.addProperty("health",health.getInt(item));
        row.addProperty("fire",item.getRemainingFireTicks()); row.addProperty("removed",item.removed); rows.add(row);
      }
      fireOut.add(rows);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(fireOut));
  }
}
