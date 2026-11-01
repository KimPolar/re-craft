import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.*;
import net.minecraft.server.dedicated.DedicatedServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.*;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.item.*;
import net.minecraft.world.item.enchantment.*;
import net.minecraft.world.item.enchantment.effects.ChangeItemDamage;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;
import net.minecraft.tags.ItemTags;

public class LocationItemDamageOracle {
    static class World extends ServerLevel {
        MinecraftServer server; RandomSource random;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public MinecraftServer getServer() { return server; }
        @Override public RandomSource getRandom() { return random; }
    }
    static void set(Object target,Class<?> type,String name,Object value) throws Exception {
        var f=type.getDeclaredField(name); f.setAccessible(true); f.set(target,value);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup=VanillaRegistries.createWorldLookup();
        var bindTags=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bindTags.setAccessible(true);
        bindTags.invoke(BuiltInRegistries.ITEM.wrapAsHolder(Items.IRON_BOOTS),List.of(ItemTags.ARMOR_ENCHANTABLE));
        var reloadable=VanillaRegistries.createReloadableLookup(lookup);
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) pending.apply();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var server=(DedicatedServer)unsafe.allocateInstance(DedicatedServer.class);
        var managers=(ReloadableServerResources)unsafe.allocateInstance(ReloadableServerResources.class);
        set(managers,ReloadableServerResources.class,"fullRegistryHolder",new ReloadableServerRegistries.Holder(reloadable));
        var resources=Class.forName("net.minecraft.server.MinecraftServer$ReloadableResources");
        var constructor=resources.getDeclaredConstructors()[0]; constructor.setAccessible(true);
        set(server,MinecraftServer.class,"resources",constructor.newInstance(null,managers));
        var world=(World)unsafe.allocateInstance(World.class); world.server=server;
        var unbreaking=lookup.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(Enchantments.UNBREAKING);
        var inputs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var outputs=new JsonArray();
        for(var value:inputs) {
            var row=value.getAsJsonObject();
            var stack=new ItemStack(Items.IRON_BOOTS);
            stack.set(DataComponents.MAX_DAMAGE,row.get("max").getAsInt());
            stack.set(DataComponents.DAMAGE,row.get("damage").getAsInt());
            if(row.get("unbreakable").getAsBoolean()) stack.set(DataComponents.UNBREAKABLE,Unit.INSTANCE);
            int level=row.get("unbreaking").getAsInt(); if(level>0) stack.enchant(unbreaking,level);
            world.random=new LegacyRandomSource(row.get("seed").getAsLong());
            var broken=new boolean[]{false};
            var used=new EnchantedItemInUse(stack,EquipmentSlot.FEET,null,item -> broken[0]=true);
            new ChangeItemDamage(LevelBasedValue.constant(row.get("amount").getAsFloat())).apply(world,1,used,null,Vec3.ZERO);
            var result=new JsonObject(); result.addProperty("empty",stack.isEmpty());
            result.addProperty("damage",stack.isEmpty() ? -1 : stack.getDamageValue());
            result.addProperty("broken",broken[0]); result.addProperty("next",world.random.nextInt()); outputs.add(result);
        }
        Files.writeString(Path.of(args[1]),new Gson().toJson(outputs));
    }
}
