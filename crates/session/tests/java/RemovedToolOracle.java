import java.io.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.*;
import net.minecraft.world.level.block.Blocks;

public class RemovedToolOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        for (var init : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(VanillaRegistries.createReloadableLookup(lookup))) init.apply();
        var states = new net.minecraft.world.level.block.state.BlockState[]{Blocks.STONE.defaultBlockState(), Blocks.DIRT.defaultBlockState(), Blocks.OAK_STAIRS.defaultBlockState()};
        try (var out = new DataOutputStream(Files.newOutputStream(Path.of(args[0])))) {
            for (String id : new String[]{"diamond_sword", "iron_pickaxe", "shears"}) {
                var stack = new ItemStack(BuiltInRegistries.ITEM.getValue(Identifier.withDefaultNamespace(id)));
                stack.remove(DataComponents.TOOL);
                var encoded = ItemStack.CODEC.encodeStart(lookup.createSerializationContext(NbtOps.INSTANCE), stack).getOrThrow();
                if (!encoded.toString().contains("!minecraft:tool")) throw new IllegalStateException("missing removed component: " + encoded);
                if (stack.get(DataComponents.TOOL) != null) throw new IllegalStateException("tool was not removed");
                for (var state : states) {
                    out.writeInt(Float.floatToRawIntBits(stack.getDestroySpeed(state)));
                    out.writeBoolean(!state.requiresCorrectToolForDrops() || stack.isCorrectToolForDrops(state));
                }
            }
        }
    }
}
