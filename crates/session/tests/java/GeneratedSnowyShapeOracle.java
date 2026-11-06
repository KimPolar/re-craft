import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.*;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;

public class GeneratedSnowyShapeOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        bind.setAccessible(true);
        var pos = new BlockPos(8, 8, 8);
        out.println("BEGIN_RESULT");
        for (boolean tagged : new boolean[]{false, true}) {
            for (Block neighbor : new Block[]{Blocks.STONE, Blocks.WATER, Blocks.SNOW})
                bind.invoke(neighbor.builtInRegistryHolder(), tagged ? List.of(BlockTags.SNOW) : List.of());
            for (Block block : new Block[]{Blocks.GRASS_BLOCK, Blocks.PODZOL, Blocks.MYCELIUM})
                for (boolean snowy : new boolean[]{false, true})
                    for (Direction direction : Direction.values())
                        for (Block neighbor : new Block[]{Blocks.STONE, Blocks.WATER, Blocks.SNOW}) {
                            var state = block.defaultBlockState().setValue(BlockStateProperties.SNOWY, snowy);
                            var random = RandomSource.create(42);
                            var next = state.updateShape(null, null, pos, direction, pos.relative(direction), neighbor.defaultBlockState(), random);
                            out.printf("%s %s %s %s %s %s %d%n", tagged,
                                BuiltInRegistries.BLOCK.getKey(block).getPath(), snowy,
                                direction.getSerializedName(), BuiltInRegistries.BLOCK.getKey(neighbor).getPath(),
                                next.getValue(BlockStateProperties.SNOWY), random.nextLong());
                        }
        }
        out.println("END_RESULT");
    }
}
