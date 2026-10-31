import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.HolderSet;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.Tool;
import net.minecraft.world.level.block.Blocks;

public class CustomToolOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createWorldLookup();
        for (var init : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(VanillaRegistries.createReloadableLookup(lookup))) init.apply();
        var stone = HolderSet.direct(Blocks.STONE.builtInRegistryHolder());
        var stoneAndDirt = HolderSet.direct(Blocks.STONE.builtInRegistryHolder(), Blocks.DIRT.builtInRegistryHolder());
        var dirt = HolderSet.direct(Blocks.DIRT.builtInRegistryHolder());
        var tools = List.of(
            new Tool(List.of(new Tool.Rule(stoneAndDirt, Optional.of(8.0f), Optional.empty()),
                             new Tool.Rule(stone, Optional.of(3.0f), Optional.of(true)),
                             new Tool.Rule(stone, Optional.of(1.0f), Optional.of(false))), 1.0f, 2, true),
            new Tool(List.of(new Tool.Rule(dirt, Optional.of(5.0f), Optional.of(false))), 2.0f, 0, false),
            new Tool(List.of(), 0.75f, 1, true));
        var states = new net.minecraft.world.level.block.state.BlockState[]{Blocks.STONE.defaultBlockState(), Blocks.DIRT.defaultBlockState(), Blocks.OAK_STAIRS.defaultBlockState()};
        var stacks = new ListTag();
        try (var out = new DataOutputStream(Files.newOutputStream(Path.of(args[1])))) {
            for (var item : List.of(Items.DIAMOND_PICKAXE, Items.APPLE)) for (var tool : tools) {
                var stack = new ItemStack(item, item == Items.APPLE ? 4 : 1);
                stack.set(DataComponents.TOOL, tool);
                stacks.add(ItemStack.CODEC.encodeStart(lookup.createSerializationContext(NbtOps.INSTANCE), stack).getOrThrow());
                for (var state : states) {
                    out.writeInt(Float.floatToRawIntBits(stack.getDestroySpeed(state)));
                    out.writeBoolean(!state.requiresCorrectToolForDrops() || stack.isCorrectToolForDrops(state));
                    out.writeInt(tool.damagePerBlock());
                    out.writeBoolean(tool.canDestroyBlocksInCreative());
                }
            }
        }
        var root = new CompoundTag(); root.put("stacks", stacks);
        NbtIo.write(root, Path.of(args[0]));
    }
}
