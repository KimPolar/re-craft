import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.ReloadableServerRegistries;
import net.minecraft.server.ReloadableServerResources;
import net.minecraft.server.dedicated.DedicatedServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.RandomSource;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.Items;
import net.minecraft.world.item.enchantment.Enchantment;
import net.minecraft.world.item.enchantment.EnchantmentHelper;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import com.mojang.serialization.JsonOps;

/** Exercise the installed 26.3 item enchantment iteration and random source. */
public class MultipleEnchantmentEffectsOracle {
    static class World extends ServerLevel {
        MinecraftServer server;
        RandomSource random;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public MinecraftServer getServer() { return server; }
        @Override public RandomSource getRandom() { return random; }
    }

    static void set(Object target, Class<?> type, String name, Object value) throws Exception {
        var field=type.getDeclaredField(name); field.setAccessible(true); field.set(target,value);
    }

    static Holder<Enchantment> enchantment(JsonObject base, JsonObject spec, String name,
                                            net.minecraft.core.HolderLookup.Provider lookup) {
        var definition=base.deepCopy();
        var description=new JsonObject(); description.addProperty("text",name);
        definition.add("description",description);
        var effects=new JsonObject();
        for (String kind:List.of("item_damage","repair_with_xp")) {
            if (spec.has(kind)) {
                var entry=new JsonObject(); entry.add("effect",spec.get(kind).deepCopy());
                var entries=new JsonArray(); entries.add(entry);
                effects.add("minecraft:"+kind,entries);
            }
        }
        definition.add("effects",effects);
        return Holder.direct(Enchantment.DIRECT_CODEC
            .parse(lookup.createSerializationContext(JsonOps.INSTANCE),definition).getOrThrow());
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup=VanillaRegistries.createWorldLookup();
        var reloadable=VanillaRegistries.createReloadableLookup(lookup);
        for (var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) pending.apply();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var server=(DedicatedServer)unsafe.allocateInstance(DedicatedServer.class);
        var managers=(ReloadableServerResources)unsafe.allocateInstance(ReloadableServerResources.class);
        set(managers,ReloadableServerResources.class,"fullRegistryHolder",new ReloadableServerRegistries.Holder(reloadable));
        var resources=Class.forName("net.minecraft.server.MinecraftServer$ReloadableResources");
        var constructor=resources.getDeclaredConstructors()[0]; constructor.setAccessible(true);
        set(server,MinecraftServer.class,"resources",constructor.newInstance(null,managers));
        var world=(World)unsafe.allocateInstance(World.class); world.server=server;
        JsonObject base;
        try (var stream=MultipleEnchantmentEffectsOracle.class.getResourceAsStream("/data/minecraft/enchantment/mending.json")) {
            base=JsonParser.parseString(new String(stream.readAllBytes(),java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject();
        }
        var inputs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var outputs=new JsonArray();
        for (var value:inputs) {
            var row=value.getAsJsonObject();
            var stack=new ItemStack(Items.IRON_PICKAXE);
            stack.enchant(enchantment(base,row.getAsJsonObject("first"),"first",lookup),1);
            stack.enchant(enchantment(base,row.getAsJsonObject("second"),"second",lookup),1);
            int amount=row.get("amount").getAsInt(); long seed=row.get("seed").getAsLong();
            world.random=new LegacyRandomSource(seed);
            int damage=EnchantmentHelper.processDurabilityChange(world,stack,amount);
            int damageNext=world.random.nextInt();
            world.random=new LegacyRandomSource(seed);
            int repair=EnchantmentHelper.modifyDurabilityToRepairFromXp(world,stack,amount);
            var out=new JsonObject(); out.addProperty("damage",damage); out.addProperty("damage_next",damageNext);
            out.addProperty("repair",repair); out.addProperty("repair_next",world.random.nextInt()); outputs.add(out);
        }
        Files.writeString(Path.of(args[1]),new Gson().toJson(outputs));
    }
}
