import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate.StructureBlockInfo;

public class BlockAgeProcessorOracle {
    static String key(BlockState state) {
        var values = new TreeMap<String, String>();
        state.getValues().forEach(entry -> values.put(entry.property().getName(), entry.valueName()));
        var out = new StringBuilder(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()).append('|');
        for (var entry : values.entrySet()) out.append(entry.getKey()).append('=').append(entry.getValue()).append(';');
        return out.toString();
    }
    public static void main(String[] args) throws Exception {
        var out = System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class); bind.setAccessible(true);
        var stairs = List.of(Blocks.STONE_BRICK_STAIRS, Blocks.OAK_STAIRS);
        var slabs = List.of(Blocks.STONE_SLAB, Blocks.STONE_BRICK_SLAB);
        var walls = List.of(Blocks.STONE_BRICK_WALL);
        for (var block : stairs) bind.invoke(block.builtInRegistryHolder(), List.of(BlockTags.STAIRS));
        for (var block : slabs) bind.invoke(block.builtInRegistryHolder(), List.of(BlockTags.SLABS));
        for (var block : walls) bind.invoke(block.builtInRegistryHolder(), List.of(BlockTags.WALLS));
        var blocks = new ArrayList<Block>(List.of(Blocks.STONE, Blocks.STONE_BRICKS,
                Blocks.CHISELED_STONE_BRICKS, Blocks.OBSIDIAN, Blocks.NETHERRACK));
        blocks.addAll(stairs); blocks.addAll(slabs); blocks.addAll(walls);
        float[] mossiness = {0.0f, 0.2f, 0.8f, 1.0f};
        for (int m = 0; m < mossiness.length; m++) {
            var processor = new BlockAgeProcessor(mossiness[m]);
            for (var block : blocks) for (var state : block.getStateDefinition().getPossibleStates())
                for (int i = 0; i < 13; i++) {
                    var pos = new BlockPos(i * 17 - 95, i * 29 - 64, 113 - i * 23);
                    var nbt = new CompoundTag(); nbt.putInt("test", 17);
                    var info = new StructureBlockInfo(pos, state, nbt);
                    var result = processor.processBlock(null, pos, pos, pos, info, new StructurePlaceSettings());
                    if (result == null || !result.pos().equals(pos) || !nbt.equals(result.nbt()))
                        throw new AssertionError("aging changed position or NBT");
                    out.printf("A\t%d\t%d\t%d\t%d\t%s\t%s%n", m, pos.getX(), pos.getY(), pos.getZ(), key(state), key(result.state()));
                }
        }
    }
}
