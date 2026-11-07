import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.FluidTags;
import net.minecraft.tags.TagLoader;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class ShipwreckDraftOracle {
    static String key(BlockState state) {
        var properties = new TreeMap<String, String>();
        state.getValues().forEach(value -> properties.put(value.property().getName(), value.valueName()));
        var entries = new ArrayList<String>();
        properties.forEach((name, value) -> entries.add(name + "=" + value));
        return BuiltInRegistries.BLOCK.getKey(state.getBlock()) + "|" + String.join(",", entries);
    }
    static BlockState original(BlockPos pos) {
        if (pos.getY() < 0 || pos.getY() >= 16) return Blocks.AIR.defaultBlockState();
        return pos.getY() < 8 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState();
    }
    public static void main(String[] names) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var fences = new ArrayList<net.minecraft.core.Holder<Block>>();
        var woodenFences = new ArrayList<net.minecraft.core.Holder<Block>>();
        var shulkers = new ArrayList<net.minecraft.core.Holder<Block>>();
        for (var block : BuiltInRegistries.BLOCK) {
            var name = BuiltInRegistries.BLOCK.getKey(block).toString();
            if (name.endsWith("_fence")) {
                fences.add(block.builtInRegistryHolder());
                if (!name.equals("minecraft:nether_brick_fence")) woodenFences.add(block.builtInRegistryHolder());
            }
            if (name.endsWith("shulker_box")) shulkers.add(block.builtInRegistryHolder());
        }
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
            BlockTags.FENCES, fences, BlockTags.WOODEN_FENCES, woodenFences, BlockTags.SHULKER_BOXES, shulkers))).apply();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(
            FluidTags.WATER, List.of(Fluids.WATER.builtInRegistryHolder(), Fluids.FLOWING_WATER.builtInRegistryHolder()),
            FluidTags.LAVA, List.of(Fluids.LAVA.builtInRegistryHolder(), Fluids.FLOWING_LAVA.builtInRegistryHolder())))).apply();
        for (String name : names) {
            var template = new StructureTemplate();
            try (var stream = ShipwreckDraftOracle.class.getClassLoader().getResourceAsStream("data/minecraft/structure/shipwreck/" + name + ".nbt")) {
                if (stream == null) throw new AssertionError("missing template " + name);
                template.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(stream, NbtAccounter.unlimitedHeap()));
            }
            for (var rotation : Rotation.values()) {
                var blocks = new HashMap<BlockPos, BlockState>();
                RandomSource random = new LegacyRandomSource(42);
                java.util.function.Function<BlockPos, BlockState> get = pos -> blocks.getOrDefault(pos, original(pos));
                var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, args) -> switch (method.getName()) {
                    case "getBlockState" -> get.apply((BlockPos) args[0]);
                    case "getFluidState" -> get.apply((BlockPos) args[0]).getFluidState();
                    case "setBlock", "setBlockAndUpdate" -> {
                        var pos = (BlockPos) args[0];
                        if (pos.getY() >= 0 && pos.getY() < 16) blocks.put(pos.immutable(), (BlockState) args[1]);
                        yield true;
                    }
                    case "scheduleTick", "updateNeighborsAt" -> null;
                    case "getBlockEntity" -> null;
                    case "isClientSide" -> false;
                    case "getRandom" -> random;
                    default -> throw new AssertionError("unexpected call " + method);
                });
                var settings = new StructurePlaceSettings().setRotation(rotation)
                    .setRotationPivot(new BlockPos(4, 0, 15))
                    .setBoundingBox(new BoundingBox(-16, 0, -16, 31, 15, 31))
                    .addProcessor(BlockIgnoreProcessor.STRUCTURE_AND_AIR).setIgnoreEntities(true);
                if (!template.placeInWorld(world, new BlockPos(0, 5, 0), BlockPos.ZERO, settings, random, 2)) throw new AssertionError("placement failed " + name);
                out.println("C\t" + name + "\t" + rotation.ordinal());
                for (var entry : blocks.entrySet()) {
                    var pos = entry.getKey();
                    if (!entry.getValue().equals(original(pos)))
                        out.println("B\t" + pos.getX() + "\t" + pos.getY() + "\t" + pos.getZ() + "\t" + key(entry.getValue()));
                }
            }
        }
    }
}
