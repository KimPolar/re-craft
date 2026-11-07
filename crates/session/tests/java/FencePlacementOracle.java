import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.*;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.phys.*;

public class FencePlacementOracle {
    static final BlockPos POS = new BlockPos(0, 8, 0);
    static class World extends ClientLevel {
        Map<BlockPos, BlockState> states;
        World() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public BlockState getBlockState(BlockPos pos) { return states.getOrDefault(pos, Blocks.STONE.defaultBlockState()); }
        @Override public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    }
    static class Context extends BlockPlaceContext {
        Context(World world, ItemStack stack, BlockHitResult hit) {
            super(world, null, InteractionHand.MAIN_HAND, stack, hit);
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup = net.minecraft.data.registries.VanillaRegistries.createWorldLookup();
        var reloadable = net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) pending.apply();
        var fences = new ArrayList<Holder<Block>>(); var wooden = new ArrayList<Holder<Block>>();
        var shulkers = new ArrayList<Holder<Block>>();
        for (var block : BuiltInRegistries.BLOCK) {
            var id = BuiltInRegistries.BLOCK.getKey(block).toString();
            if (id.endsWith("_fence")) {
                fences.add(block.builtInRegistryHolder());
                if (!id.equals("minecraft:nether_brick_fence")) wooden.add(block.builtInRegistryHolder());
            }
            if (id.endsWith("shulker_box")) shulkers.add(block.builtInRegistryHolder());
        }
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.FENCES, fences, BlockTags.WOODEN_FENCES, wooden, BlockTags.SHULKER_BOXES, shulkers))).apply();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world = (World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
        var result = new JsonArray();
        for (var fence : fences) for (var fluid : List.of("air", "water", "flowing"))
            for (var direction : List.of(Direction.NORTH, Direction.EAST, Direction.SOUTH, Direction.WEST))
                for (var neighbor : List.of("air", "stone", "oak_planks", "oak_fence", "nether_brick_fence", "oak_fence_gate", "oak_leaves", "barrier", "pumpkin")) {
                    var id = BuiltInRegistries.BLOCK.getKey(fence.value());
                    var other = BuiltInRegistries.BLOCK.getValue(Identifier.withDefaultNamespace(neighbor)).defaultBlockState();
                    world.states = new HashMap<>();
                    for (var d : Direction.Plane.HORIZONTAL) world.states.put(POS.relative(d), Blocks.AIR.defaultBlockState());
                    world.states.put(POS.relative(direction), other);
                    world.states.put(POS, fluid.equals("air") ? Blocks.AIR.defaultBlockState()
                        : Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, fluid.equals("water") ? 0 : 1));
                    var stack = new ItemStack(fence.value().asItem());
                    var hit = new BlockHitResult(Vec3.atCenterOf(POS.below()), Direction.UP, POS.below(), false);
                    var context = new Context(world, stack, hit);
                    if (!context.getClickedPos().equals(POS)) throw new AssertionError("bad placement context");
                    var placed = fence.value().getStateForPlacement(context);
                    if (placed == null) throw new AssertionError("missing placement state " + id);
                    var row = new JsonObject(); row.addProperty("fence", id.toString());
                    row.addProperty("fluid", fluid); row.addProperty("neighbor", neighbor);
                    row.addProperty("direction", direction.getName());
                    var properties = new JsonObject();
                    placed.getValues().forEach(value -> properties.addProperty(value.property().getName(), value.valueName()));
                    row.add("properties", properties); result.add(row);
                }
        Files.writeString(Path.of(args[0]), result.toString());
    }
}
