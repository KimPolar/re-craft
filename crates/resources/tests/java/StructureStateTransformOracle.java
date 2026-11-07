import com.google.gson.JsonObject;
import java.util.*;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;

public class StructureStateTransformOracle {
    static JsonObject properties(BlockState state) {
        var result = new JsonObject();
        state.getValues().forEach(value -> result.addProperty(value.property().getName(), value.valueName()));
        return result;
    }
    public static void main(String[] args) throws Exception {
        var output=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var states = new LinkedHashSet<BlockState>();
        try (var zip = new ZipFile(args[0])) {
            var entries = zip.entries();
            while (entries.hasMoreElements()) {
                var entry = entries.nextElement();
                if (!entry.getName().startsWith("data/minecraft/structure/underwater_ruin/") || !entry.getName().endsWith(".nbt")) continue;
                try (var input = zip.getInputStream(entry)) {
                    var palette = NbtIo.readCompressed(input,NbtAccounter.unlimitedHeap()).getListOrEmpty("palette");
                    for (int i=0;i<palette.size();i++) {
                        var state = NbtUtils.readBlockState(BuiltInRegistries.BLOCK,palette.getCompoundOrEmpty(i));
                        if (state.getBlock()!=Blocks.STRUCTURE_BLOCK) states.add(state);
                    }
                }
            }
        }
        for (var block : BuiltInRegistries.BLOCK) {
            if (block instanceof GlazedTerracottaBlock || block==Blocks.SANDSTONE_STAIRS || block==Blocks.STONE_BRICK_STAIRS || block==Blocks.IRON_CHAIN)
                states.addAll(block.getStateDefinition().getPossibleStates());
        }
        for (var block : List.of(Blocks.POTENT_SULFUR, Blocks.SULFUR_SPIKE))
            states.addAll(block.getStateDefinition().getPossibleStates());
        states.addAll(Blocks.DARK_OAK_TRAPDOOR.getStateDefinition().getPossibleStates());
        for (var block : List.of(Blocks.STONE_BRICK_WALL, Blocks.MOSSY_STONE_BRICK_WALL,
                Blocks.COBBLESTONE_WALL, Blocks.MOSSY_COBBLESTONE_WALL,
                Blocks.BLACKSTONE_WALL, Blocks.POLISHED_BLACKSTONE_BRICK_WALL))
            states.addAll(block.getStateDefinition().getPossibleStates());
        for (var state : states) for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
            var record=new JsonObject();
            record.addProperty("id",BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
            record.addProperty("mirror",mirror.ordinal());
            record.addProperty("rotation",rotation.ordinal());
            record.add("input",properties(state));
            record.add("result",properties(state.mirror(mirror).rotate(rotation)));
            output.println("STATE\t"+record);
        }
    }
}
