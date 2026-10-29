import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.ReloadableServerRegistries;
import net.minecraft.server.ReloadableServerResources;
import net.minecraft.server.dedicated.DedicatedServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.enchantment.Enchantment;
import net.minecraft.world.item.enchantment.EnchantmentEffectComponents;
import net.minecraft.world.item.enchantment.Enchantments;
import net.minecraft.world.item.enchantment.effects.EnchantmentValueEffect;
import net.minecraft.world.level.levelgen.LegacyRandomSource;

/** Invoke the installed 26.3 ExperienceOrb repair implementation. */
public class ExperienceRepairOracle {
    static class TestLevel extends ServerLevel {
        MinecraftServer server;
        RandomSource random;
        TestLevel() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public MinecraftServer getServer() { return server; }
        @Override public RandomSource getRandom() { return random; }
    }
    static class TestPlayer extends ServerPlayer {
        TestLevel world;
        RandomSource random;
        ItemStack[] equipment;
        TestPlayer() { super(null,null,null,null); }
        @Override public ServerLevel level() { return world; }
        @Override public RandomSource getRandom() { return random; }
        @Override public ItemStack getItemBySlot(EquipmentSlot slot) {
            return slot.ordinal() < equipment.length ? equipment[slot.ordinal()] : ItemStack.EMPTY;
        }
    }
    static void set(Object target, Class<?> type, String name, Object value) throws Exception {
        Field field=type.getDeclaredField(name); field.setAccessible(true); field.set(target,value);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var worldLookup=VanillaRegistries.createWorldLookup();
        var reloadable=VanillaRegistries.createReloadableLookup(worldLookup);
        for (var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) pending.apply();
        Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var server=(DedicatedServer)unsafe.allocateInstance(DedicatedServer.class);
        var managers=(ReloadableServerResources)unsafe.allocateInstance(ReloadableServerResources.class);
        set(managers,ReloadableServerResources.class,"fullRegistryHolder",new ReloadableServerRegistries.Holder(reloadable));
        var resources=Class.forName("net.minecraft.server.MinecraftServer$ReloadableResources");
        var constructor=resources.getDeclaredConstructors()[0]; constructor.setAccessible(true);
        set(server,MinecraftServer.class,"resources",constructor.newInstance(null,managers));
        var level=(TestLevel)unsafe.allocateInstance(TestLevel.class);
        level.server=server; level.random=new LegacyRandomSource(1);
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.world=level;
        var orb=(ExperienceOrb)unsafe.allocateInstance(ExperienceOrb.class);
        Method repair=ExperienceOrb.class.getDeclaredMethod("repairPlayerItems",ServerPlayer.class,int.class);
        repair.setAccessible(true);
        Item[] types={Items.DIAMOND_PICKAXE,Items.DIAMOND_SWORD,Items.DIAMOND_BOOTS,
            Items.DIAMOND_LEGGINGS,Items.DIAMOND_CHESTPLATE,Items.DIAMOND_HELMET};
        var mending=worldLookup.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(Enchantments.MENDING);
        var inputs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var results=new JsonArray();
        for (var value:inputs) {
            var c=value.getAsJsonObject(); player.random=new LegacyRandomSource(c.get("seed").getAsLong());
            player.equipment=new ItemStack[types.length]; var damages=c.getAsJsonArray("damage");
            var enchanted=c.getAsJsonArray("mending");
            for (int i=0;i<types.length;i++) {
                var item=new ItemStack(types[i]); item.setDamageValue(damages.get(i).getAsInt());
                if (enchanted.get(i).getAsBoolean()) item.enchant(mending,1);
                player.equipment[i]=item;
            }
            int remaining=(int)repair.invoke(orb,player,c.get("xp").getAsInt());
            var row=new JsonObject(); var resultDamage=new JsonArray();
            for (var item:player.equipment) resultDamage.add(item.getDamageValue());
            row.add("damage",resultDamage); row.addProperty("remaining",remaining);
            row.addProperty("next_random",player.random.nextInt()); results.add(row);
        }
        Files.writeString(Path.of(args[1]),new Gson().toJson(results));
        var sample=new ItemStack(Items.DIAMOND_PICKAXE);
        sample.setDamageValue(21); sample.enchant(mending,1);
        var encoded=ItemStack.CODEC.encodeStart(worldLookup.createSerializationContext(NbtOps.INSTANCE),sample).getOrThrow();
        NbtIo.write((CompoundTag)encoded,Path.of(args[2]));
        var effectCases=JsonParser.parseString(Files.readString(Path.of(args[3]))).getAsJsonArray();
        var effectResults=new JsonArray();
        for (var value:effectCases) {
            var c=value.getAsJsonObject();
            var effect=EnchantmentValueEffect.CODEC.parse(JsonOps.INSTANCE,c.get("effect")).getOrThrow();
            float result=effect.process(c.get("level").getAsInt(),new LegacyRandomSource(1),c.get("value").getAsFloat());
            effectResults.add(Float.floatToRawIntBits(result));
        }
        Files.writeString(Path.of(args[4]),new Gson().toJson(effectResults));
        var keyResults=new JsonArray();
        try (var stream=ExperienceRepairOracle.class.getResourceAsStream("/data/minecraft/enchantment/mending.json")) {
            var original=JsonParser.parseString(new String(stream.readAllBytes(),java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject();
            original.addProperty("supported_items","minecraft:diamond_pickaxe");
            for (int mode=0;mode<6;mode++) {
                var definition=original.deepCopy(); var effects=definition.getAsJsonObject("effects");
                var value=effects.get("minecraft:repair_with_xp").deepCopy();
                if (mode==1 || mode==2 || mode==5) effects.remove("minecraft:repair_with_xp");
                if (mode==1 || mode==3 || mode==5) effects.add("repair_with_xp",value.deepCopy());
                if (mode==2 || mode==4 || mode==5) effects.add(":repair_with_xp",value.deepCopy());
                var result=Enchantment.DIRECT_CODEC.parse(worldLookup.createSerializationContext(JsonOps.INSTANCE),definition);
                keyResults.add(result.result().map(e -> e.getEffects(EnchantmentEffectComponents.REPAIR_WITH_XP).size()==1).orElse(false));
            }
        }
        Files.writeString(Path.of(args[5]),new Gson().toJson(keyResults));
    }
}
