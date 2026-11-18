import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.phys.shapes.Shapes;
import net.minecraft.world.phys.shapes.BooleanOp;

/** Check whether full wall collision shapes are equivalent to an open fluid face. */
public class WallFluidShapeOracle {
    public static void main(String[] args) {
        var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        long count=0;
        for(var wall:List.of(Blocks.STONE_BRICK_WALL,Blocks.MOSSY_STONE_BRICK_WALL))
            for(var state:wall.getStateDefinition().getPossibleStates()) {
                boolean wet=state.getValue(WallBlock.WATERLOGGED);
                if(state.getFluidState().isSource()!=wet || !state.isSolid())throw new AssertionError("wall fluid/solidity: "+state);
                var container=(LiquidBlockContainer)wall;
                for(var fluid:List.of(Fluids.WATER,Fluids.FLOWING_WATER,Fluids.LAVA,Fluids.FLOWING_LAVA))
                    if(container.canPlaceLiquid(null,EmptyBlockGetter.INSTANCE,BlockPos.ZERO,state,fluid)!=(fluid==Fluids.WATER))
                        throw new AssertionError("wall fluid placement: "+state+" / "+fluid);
                var shape=state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO);
                for (var direction : Direction.values())
                    if (state.isFaceSturdy(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, direction))
                        throw new AssertionError("wall has sturdy face: " + state + " / " + direction);
                for(var neighbor:List.of(Blocks.AIR,Blocks.WATER,Blocks.STONE,Blocks.OAK_FENCE,Blocks.STONE_SLAB,
                        Blocks.OAK_STAIRS,Blocks.LADDER,Blocks.OAK_LEAVES,Blocks.GLOW_LICHEN,
                        Blocks.POINTED_DRIPSTONE,Blocks.STONE_BRICK_WALL))
                    for(var other:neighbor.getStateDefinition().getPossibleStates()) {
                        var otherShape=other.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO);
                        for(var direction:Direction.values()) {
                            if(Shapes.mergedFaceOccludes(shape,otherShape,direction)!=Shapes.mergedFaceOccludes(Shapes.empty(),otherShape,direction)
                                    || Shapes.mergedFaceOccludes(otherShape,shape,direction)!=Shapes.mergedFaceOccludes(otherShape,Shapes.empty(),direction))
                                throw new AssertionError("wall face differs: "+state+" / "+other+" / "+direction);
                            count++;
                        }
                    }
            }
        var materials = new ArrayList<String>();
        for (var id : BuiltInRegistries.BLOCK.keySet()) {
            var block = BuiltInRegistries.BLOCK.getValue(id);
            if (!(block instanceof WallBlock)) continue;
            if (block.getClass() != WallBlock.class) throw new AssertionError("custom wall class: " + id);
            materials.add(id.toString());
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var reference = Blocks.STONE_BRICK_WALL.defaultBlockState()
                    .setValue(WallBlock.NORTH, state.getValue(WallBlock.NORTH))
                    .setValue(WallBlock.EAST, state.getValue(WallBlock.EAST))
                    .setValue(WallBlock.SOUTH, state.getValue(WallBlock.SOUTH))
                    .setValue(WallBlock.WEST, state.getValue(WallBlock.WEST))
                    .setValue(WallBlock.UP, state.getValue(WallBlock.UP))
                    .setValue(WallBlock.WATERLOGGED, state.getValue(WallBlock.WATERLOGGED));
                if (Shapes.joinIsNotEmpty(state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO),
                        reference.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO), BooleanOp.NOT_SAME))
                    throw new AssertionError("wall material collision differs: " + state);
                if (state.getFluidState().isSource() != state.getValue(WallBlock.WATERLOGGED))
                    throw new AssertionError("wall material fluid differs: " + state);
            }
        }
        Collections.sort(materials);
        out.println("BEGIN_RESULT"); out.println("checked " + count);
        for (var material : materials) out.println("material " + material);
        out.println("END_RESULT");
    }
}
