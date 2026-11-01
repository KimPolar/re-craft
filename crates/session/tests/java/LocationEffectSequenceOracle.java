import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.*;
import net.minecraft.server.dedicated.DedicatedServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.*;
import net.minecraft.world.item.*;
import net.minecraft.world.item.enchantment.*;
import net.minecraft.world.item.enchantment.effects.EnchantmentLocationBasedEffect;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.phys.Vec3;

public class LocationEffectSequenceOracle {
    static class World extends ServerLevel {
        MinecraftServer server; RandomSource random; BlockState state;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public MinecraftServer getServer() { return server; }
        @Override public RandomSource getRandom() { return random; }
        @Override public BlockState getBlockState(BlockPos pos) { return state; }
        @Override public boolean isLoaded(BlockPos pos) { return true; }
    }
    static class TestPlayer extends Player {
        World world; boolean ground, passenger; Abilities abilities; AttributeMap attributes;
        Map<Enchantment,Set<EnchantmentLocationBasedEffect>> active;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public ServerLevel level() { return world; }
        @Override public boolean onGround() { return ground; }
        @Override public boolean isFallFlying() { return false; }
        @Override public Abilities getAbilities() { return abilities; }
        @Override public Entity getVehicle() { return passenger ? this : null; }
        @Override public Vec3 position() { return new Vec3(0.5,1.0,0.5); }
        @Override public BlockPos getBlockPosBelowThatAffectsMyMovement() { return BlockPos.ZERO; }
        @Override public AttributeMap getAttributes() { return attributes; }
        @Override public double getAttributeValue(Holder<Attribute> attr) { return attributes.getValue(attr); }
        @Override public Map<Enchantment,Set<EnchantmentLocationBasedEffect>> activeLocationDependentEnchantments(EquipmentSlot slot) { return active; }
    }
    static void set(Object target,Class<?> type,String name,Object value) throws Exception {
        var f=type.getDeclaredField(name); f.setAccessible(true); f.set(target,value);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup=VanillaRegistries.createWorldLookup();
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
        bind.invoke(Blocks.SOUL_SAND.builtInRegistryHolder(),List.of(BlockTags.SOUL_SPEED_BLOCKS));
        bind.invoke(Blocks.SOUL_SOIL.builtInRegistryHolder(),List.of(BlockTags.SOUL_SPEED_BLOCKS));
        bind.invoke(BuiltInRegistries.ITEM.wrapAsHolder(Items.IRON_BOOTS),List.of(ItemTags.ARMOR_ENCHANTABLE));
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
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.world=world;
        var soul=lookup.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(Enchantments.SOUL_SPEED);
        var inputs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output=new JsonArray();
        for(var input:inputs) {
            var scenario=input.getAsJsonObject(); int level=scenario.get("level").getAsInt();
            player.attributes=new AttributeMap(Player.createAttributes().build()); player.active=new IdentityHashMap<>(); player.abilities=new Abilities();
            world.random=new LegacyRandomSource(scenario.get("seed").getAsLong());
            var stack=new ItemStack(Items.IRON_BOOTS); stack.enchant(soul,level);
            var item=new EnchantedItemInUse(stack,EquipmentSlot.FEET,null,broken -> {});
            var rows=new JsonArray();
            for(var value:scenario.getAsJsonArray("steps")) {
                var step=value.getAsJsonObject(); player.ground=step.get("ground").getAsBoolean();
                player.passenger=step.get("passenger").getAsBoolean(); player.abilities.flying=step.get("flying").getAsBoolean();
                world.state=(step.get("block").getAsString().equals("soul_sand") ? Blocks.SOUL_SAND : Blocks.STONE).defaultBlockState();
                soul.value().runLocationChangedEffects(world,level,item,player);
                var row=new JsonObject(); row.addProperty("speed",player.attributes.getValue(Attributes.MOVEMENT_SPEED));
                row.addProperty("efficiency",player.attributes.getValue(Attributes.MOVEMENT_EFFICIENCY));
                row.addProperty("damage",stack.getDamageValue()); row.addProperty("next",world.random.nextInt()); rows.add(row);
            }
            output.add(rows);
        }
        Files.writeString(Path.of(args[1]),new Gson().toJson(output));
    }
}
