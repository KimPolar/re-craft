import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.phys.shapes.Shapes;

public class SpeleothemFluidShapeOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        int count = 0;
        for (var spike : List.of(Blocks.POINTED_DRIPSTONE, Blocks.SULFUR_SPIKE)) {
            for (var state : spike.getStateDefinition().getPossibleStates()) {
                if (!state.getValue(BlockStateProperties.WATERLOGGED)) continue;
                if (state.getValue(BlockStateProperties.VERTICAL_DIRECTION) != Direction.UP
                        || state.getValues().noneMatch(entry -> entry.property().getName().equals("thickness")
                                && entry.valueName().equals("tip"))) continue;
                if (!state.getFluidState().isSource()) throw new AssertionError(state);
                var shape = state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
                for (var neighbor : List.of(Blocks.AIR, Blocks.WATER, Blocks.STONE,
                        Blocks.OAK_FENCE, Blocks.STONE_SLAB, Blocks.OAK_STAIRS,
                        Blocks.LADDER, Blocks.OAK_LEAVES)) {
                    for (var other : neighbor.getStateDefinition().getPossibleStates()) {
                        var otherShape = other.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
                        for (var direction : Direction.values()) {
                            if (Shapes.mergedFaceOccludes(shape, otherShape, direction)
                                    != Shapes.mergedFaceOccludes(Shapes.empty(), otherShape, direction)
                                    || Shapes.mergedFaceOccludes(otherShape, shape, direction)
                                    != Shapes.mergedFaceOccludes(otherShape, Shapes.empty(), direction)) {
                                throw new AssertionError(state + " / " + other + " / " + direction);
                            }
                            count++;
                        }
                    }
                }
            }
        }
        out.println("BEGIN_RESULT");
        out.println("checked " + count);
        out.println("END_RESULT");
    }
}
