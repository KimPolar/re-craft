import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.TagKey;
import net.minecraft.world.effect.*;
import net.minecraft.world.entity.EntityEquipment;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.*;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.Tool;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.material.Fluid;

public class MiningEffectsOracle {
    static class TestPlayer extends Player {
        boolean grounded, underwater;
        AttributeMap attributes;
        Map<Holder<MobEffect>, MobEffectInstance> effects;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean onGround() { return grounded; }
        @Override public boolean isEyeInFluid(TagKey<Fluid> tag) { return underwater; }
        @Override public boolean hasEffect(Holder<MobEffect> effect) { return effects.containsKey(effect); }
        @Override public MobEffectInstance getEffect(Holder<MobEffect> effect) { return effects.get(effect); }
        @Override public double getAttributeValue(Holder<Attribute> attribute) { return attributes.getValue(attribute); }
        @Override public AttributeInstance getAttribute(Holder<Attribute> attribute) { return attributes.getInstance(attribute); }
    }
    static void setEffect(TestPlayer player, Holder<MobEffect> id, int amplifier) {
        if (amplifier >= 0) player.effects.put(id, new MobEffectInstance(id, 100, amplifier));
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var world = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(world);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var player = (TestPlayer) ((sun.misc.Unsafe) f.get(null)).allocateInstance(TestPlayer.class);
        var inventory = new Inventory(player, new EntityEquipment());
        var inventoryField = Player.class.getDeclaredField("inventory"); inventoryField.setAccessible(true); inventoryField.set(player, inventory);
        player.attributes = new AttributeMap(Player.createAttributes().build());
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var output = new JsonArray();
        for (var entry : input) {
            var c = entry.getAsJsonObject();
            player.grounded = c.get("ground").getAsBoolean(); player.underwater = c.get("water").getAsBoolean();
            player.attributes.getInstance(Attributes.MINING_EFFICIENCY).setBaseValue(c.get("efficiency").getAsDouble());
            player.attributes.getInstance(Attributes.BLOCK_BREAK_SPEED).setBaseValue(c.get("break_speed").getAsDouble());
            player.attributes.getInstance(Attributes.SUBMERGED_MINING_SPEED).setBaseValue(c.get("submerged").getAsDouble());
            player.effects = new HashMap<>();
            setEffect(player, MobEffects.HASTE, c.get("haste").getAsInt());
            setEffect(player, MobEffects.CONDUIT_POWER, c.get("conduit").getAsInt());
            setEffect(player, MobEffects.MINING_FATIGUE, c.get("fatigue").getAsInt());
            var state = c.get("stone").getAsBoolean() ? Blocks.STONE.defaultBlockState() : Blocks.DIRT.defaultBlockState();
            ItemStack hand = ItemStack.EMPTY;
            if (c.get("tool").getAsBoolean()) {
                hand = new ItemStack(Items.IRON_PICKAXE);
                var blocks = HolderSet.direct(Blocks.STONE.builtInRegistryHolder());
                hand.set(DataComponents.TOOL, new Tool(List.of(Tool.Rule.minesAndDrops(blocks, 6.0f)), 1.0f, 1, true));
            }
            inventory.setItem(0, hand);
            var row = new JsonArray();
            row.add(Float.floatToRawIntBits(player.getDestroySpeed(state)));
            row.add(Float.floatToRawIntBits(state.getDestroyProgress(player, null, BlockPos.ZERO)));
            output.add(row);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(output));
    }
}
