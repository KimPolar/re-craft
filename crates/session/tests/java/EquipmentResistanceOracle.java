import com.google.gson.*;
import java.nio.file.*;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.*;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.TagKey;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.DamageResistant;
import net.minecraft.world.level.*;

public class EquipmentResistanceOracle {
    static class TestPlayer extends Player {
        Level world; ItemStack helmet; boolean netherite;
        TestPlayer() { super(null,null); }
        @Override public Level level() { return world; }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public ItemStack getItemBySlot(EquipmentSlot slot) { return slot==EquipmentSlot.HEAD ? helmet : ItemStack.EMPTY; }
        @Override public void onEquippedItemBroken(ItemStack item, EquipmentSlot slot) { }
        @Override public double getAttributeValue(Holder<Attribute> attribute) {
            if(attribute == Attributes.ARMOR) return helmet.isEmpty()?0:netherite?3:2;
            if(attribute == Attributes.ARMOR_TOUGHNESS) return helmet.isEmpty()?0:netherite?3:0;
            throw new IllegalStateException();
        }
        float apply(DamageSource source,float damage) { return getDamageAfterArmorAbsorb(source,damage); }
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var lookup=registries.lookupOrThrow(Registries.DAMAGE_TYPE);
        var lava=lookup.getOrThrow(DamageTypes.LAVA); var generic=lookup.getOrThrow(DamageTypes.CACTUS);
        var fire=TagKey.create(Registries.DAMAGE_TYPE,Identifier.parse("minecraft:is_fire"));
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",java.util.Collection.class); bind.setAccessible(true);
        bind.invoke(lava,java.util.List.of(fire)); bind.invoke(generic,java.util.List.of());
        var ops=RegistryOps.create(NbtOps.INSTANCE,registries);
        var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)unsafeField.get(null);
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        player.world=(ServerLevel)unsafe.allocateInstance(ServerLevel.class);
        var rows=new JsonArray();
        for(int variant=0;variant<8;variant++) for(boolean lavaSource:new boolean[]{false,true}) for(int initial:new int[]{0,164,406}) for(float damage:new float[]{4f,20f}) {
            player.netherite=variant>=4; player.helmet=new ItemStack(player.netherite?Items.NETHERITE_HELMET:Items.IRON_HELMET);
            player.helmet.set(DataComponents.DAMAGE,Math.min(initial,player.helmet.getMaxDamage()-1));
            if(variant==5) player.helmet.remove(DataComponents.DAMAGE_RESISTANT);
            if(variant==1 || variant==2 || variant==3 || variant==6 || variant==7) {
                var component=new CompoundTag();
                if(variant==7) component.putString("types","#minecraft:is_fire");
                else { var list=new ListTag(); if(variant==1) list.add(StringTag.valueOf("minecraft:lava")); if(variant==2 || variant==6) list.add(StringTag.valueOf("minecraft:cactus")); component.put("types",list); }
                player.helmet.set(DataComponents.DAMAGE_RESISTANT,DamageResistant.CODEC.parse(ops,component).getOrThrow());
            }
            var source=new DamageSource(lavaSource?lava:generic);
            float health=Math.max(100f-player.apply(source,damage),0f);
            var row=new JsonObject(); row.addProperty("variant",variant); row.addProperty("lava",lavaSource); row.addProperty("initial",initial); row.addProperty("amount",damage);
            row.addProperty("damage",player.helmet.isEmpty()?-1:player.helmet.getDamageValue()); row.addProperty("health_bits",Float.floatToRawIntBits(health)); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
