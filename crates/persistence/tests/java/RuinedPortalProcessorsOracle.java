import com.mojang.serialization.Lifecycle;
import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalPiece;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate.StructureBlockInfo;

public class RuinedPortalProcessorsOracle {
    static String key(BlockState state) {
        var values = new TreeMap<String, String>();
        state.getValues().forEach(entry -> values.put(entry.property().getName(), entry.valueName()));
        var out = new StringBuilder(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()).append('|');
        for (var e : values.entrySet()) out.append(e.getKey()).append('=').append(e.getValue()).append(';');
        return out.toString();
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class); bind.setAccessible(true);
        for (var b : List.of(Blocks.STONE_BRICK_STAIRS, Blocks.MOSSY_STONE_BRICK_STAIRS))
            bind.invoke(b.builtInRegistryHolder(), List.of(BlockTags.STAIRS));
        for (var b : List.of(Blocks.STONE_SLAB, Blocks.STONE_BRICK_SLAB, Blocks.MOSSY_STONE_BRICK_SLAB))
            bind.invoke(b.builtInRegistryHolder(), List.of(BlockTags.SLABS));
        for (var b : List.of(Blocks.STONE_BRICK_WALL, Blocks.MOSSY_STONE_BRICK_WALL))
            bind.invoke(b.builtInRegistryHolder(), List.of(BlockTags.WALLS));
        var blocks = new MappedRegistry<Block>(Registries.BLOCK, Lifecycle.stable());
        blocks.bindTags(Map.of(BlockTags.FEATURES_CANNOT_REPLACE, List.of())); blocks.freeze();
        var registries = new RegistryAccess.ImmutableRegistryAccess(List.of(blocks));
        var method = RuinedPortalPiece.class.getDeclaredMethod("makeSettings", HolderLookup.Provider.class,
            Mirror.class, Rotation.class, RuinedPortalPiece.VerticalPlacement.class, BlockPos.class, RuinedPortalPiece.Properties.class);
        method.setAccessible(true);
        var inputs = new ArrayList<BlockState>();
        for (var b : List.of(Blocks.AIR, Blocks.STRUCTURE_BLOCK, Blocks.GOLD_BLOCK, Blocks.LAVA,
                Blocks.NETHERRACK, Blocks.MAGMA_BLOCK, Blocks.OBSIDIAN, Blocks.STONE, Blocks.STONE_BRICKS,
                Blocks.STONE_BRICK_STAIRS, Blocks.STONE_SLAB, Blocks.STONE_BRICK_WALL, Blocks.IRON_BARS, Blocks.CHEST)) {
            var states = b.getStateDefinition().getPossibleStates();
            inputs.add(b.defaultBlockState());
            if (states.size() > 1) inputs.add(states.getLast());
        }
        var placements = RuinedPortalPiece.VerticalPlacement.values();
        int count = 0;
        for (int p = 0; p < placements.length; p++) for (int flags = 0; flags < 8; flags++) {
            var properties = new RuinedPortalPiece.Properties((flags & 1) != 0, 0.8f,
                    (flags & 2) != 0, false, false, (flags & 4) != 0);
            var settings = (StructurePlaceSettings)method.invoke(null, registries, Mirror.NONE, Rotation.NONE, placements[p], BlockPos.ZERO, properties);
            for (int i = 0; i < 11; i++) for (int w = 0; w < 2; w++) {
                var pos = new BlockPos(i * 17 - 75, i * 19 - 64, 87 - i * 13);
                var worldState = w == 0 ? Blocks.STONE.defaultBlockState() : Blocks.LAVA.defaultBlockState();
                var world = (LevelReader)Proxy.newProxyInstance(LevelReader.class.getClassLoader(), new Class<?>[]{LevelReader.class},
                    (proxy, m, a) -> { if (m.getName().equals("getBlockState")) return worldState; throw new UnsupportedOperationException(m.toString()); });
                for (var input : inputs) {
                    var nbt = new CompoundTag(); nbt.putInt("test", 17);
                    var original = new StructureBlockInfo(pos, input, nbt);
                    var result = original;
                    for (var processor : settings.getProcessors()) {
                        result = processor.processBlock(world, pos, pos, pos, result, settings);
                        if (result == null) break;
                    }
                    if (result != null && (!pos.equals(result.pos()) || !nbt.equals(result.nbt())))
                        throw new AssertionError("processor changed position or NBT");
                    out.printf("P\t%d\t%d\t%d\t%d\t%s\t%s%n", p, flags, i, w, key(input), result == null ? "<skip>" : key(result.state()));
                    count++;
                }
            }
        }
        out.printf("COUNT\t%d%n", count);
    }
}
