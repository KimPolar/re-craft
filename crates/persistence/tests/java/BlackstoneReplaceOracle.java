import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate.StructureBlockInfo;

public class BlackstoneReplaceOracle {
    static String key(BlockState state) {
        var values = new TreeMap<String, String>();
        state.getValues().forEach(entry -> values.put(entry.property().getName(), entry.valueName()));
        var out = new StringBuilder(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()).append('|');
        for (var entry : values.entrySet()) out.append(entry.getKey()).append('=').append(entry.getValue()).append(';');
        return out.toString();
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var processor = BlackstoneReplaceProcessor.INSTANCE;
        out.printf("TYPE\t%s%n", BuiltInRegistries.STRUCTURE_PROCESSOR.getKey(processor.codec()));
        var field = BlackstoneReplaceProcessor.class.getDeclaredField("replacements");
        field.setAccessible(true);
        @SuppressWarnings("unchecked") var replacements = (Map<Block, Block>)field.get(processor);
        var blocks = new ArrayList<>(replacements.keySet());
        blocks.addAll(List.of(Blocks.OBSIDIAN, Blocks.OAK_STAIRS, Blocks.AIR));
        var pos = new BlockPos(-21, 64, 81);
        int count = 0;
        for (var block : blocks) for (var state : block.getStateDefinition().getPossibleStates()) {
            var nbt = new CompoundTag(); nbt.putInt("test", 17);
            var result = processor.processBlock(null, pos, pos, pos,
                new StructureBlockInfo(pos, state, nbt), new StructurePlaceSettings());
            if (result == null || !result.pos().equals(pos) || !nbt.equals(result.nbt()))
                throw new AssertionError("replacement changed position or NBT");
            out.printf("B\t%s\t%s%n", key(state), key(result.state()));
            count++;
        }
        out.printf("COUNT\t%d\t%d%n", replacements.size(), count);
    }
}
