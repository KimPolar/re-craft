import java.io.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.Direction;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.WallSide;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.core.BlockPos;
import net.minecraft.world.phys.shapes.*;

public class WallShapeOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var wall = Blocks.STONE_BRICK_WALL;
        var sides = WallBlock.class.getDeclaredField("TEST_SHAPES_WALL");
        var post = WallBlock.class.getDeclaredField("TEST_SHAPE_POST");
        sides.setAccessible(true); post.setAccessible(true);
        @SuppressWarnings("unchecked")
        var sideShapes = (Map<Direction,VoxelShape>)sides.get(null);
        var postShape = (VoxelShape)post.get(null);
        Direction[] directions = {Direction.NORTH, Direction.EAST, Direction.SOUTH, Direction.WEST};
        var update = WallBlock.class.getDeclaredMethod("updateSides", BlockState.class,
            boolean.class, boolean.class, boolean.class, boolean.class, VoxelShape.class);
        var raise = WallBlock.class.getDeclaredMethod("shouldRaisePost", BlockState.class, BlockState.class, VoxelShape.class);
        var covered = WallBlock.class.getDeclaredMethod("isCovered", VoxelShape.class, VoxelShape.class);
        update.setAccessible(true); raise.setAccessible(true); covered.setAccessible(true);
        BlockState[] above = {Blocks.AIR.defaultBlockState(),
            wall.defaultBlockState().setValue(WallBlock.UP, false), wall.defaultBlockState()};
        try (var out = new DataOutputStream(Files.newOutputStream(Path.of(args[0])))) {
            for (int connection = 0; connection < 16; connection++)
                for (int shapes = 0; shapes < 32; shapes++)
                    for (var top : above) {
                        if (top.is(BlockTags.WALL_POST_OVERRIDE)) throw new AssertionError("unexpected override tag");
                        VoxelShape face = Shapes.empty();
                        for (int i = 0; i < 4; i++)
                            if ((shapes & (1 << i)) != 0) face = Shapes.or(face, sideShapes.get(directions[i]));
                        if ((shapes & 16) != 0) face = Shapes.or(face, postShape);
                        int flags = 0;
                        for (int i = 0; i < 4; i++)
                            if ((Boolean)covered.invoke(null, face, sideShapes.get(directions[i]))) flags |= 1 << i;
                        if ((Boolean)covered.invoke(null, face, postShape)) flags |= 16;
                        BlockState result = (BlockState)update.invoke(wall, wall.defaultBlockState(),
                            (connection & 1) != 0, (connection & 2) != 0,
                            (connection & 4) != 0, (connection & 8) != 0, face);
                        boolean up = (Boolean)raise.invoke(wall, result, top, face);
                        int packed = 0;
                        for (int i = 0; i < 4; i++) {
                            WallSide side = result.getValue(WallBlock.PROPERTY_BY_DIRECTION.get(directions[i]));
                            packed |= (side == WallSide.NONE ? 0 : side == WallSide.LOW ? 1 : 2) << (i * 2);
                        }
                        out.writeByte(flags);
                        out.writeByte(packed);
                        out.writeBoolean(up);
                    }
            for (int code = 0; code < 81; code++) for (int postUp = 0; postUp < 2; postUp++)
                for (int wet = 0; wet < 2; wet++) {
                    int remaining = code;
                    var state = wall.defaultBlockState().setValue(WallBlock.UP, postUp != 0)
                        .setValue(WallBlock.WATERLOGGED, wet != 0);
                    for (int i = 0; i < 4; i++) {
                        int side = remaining % 3; remaining /= 3;
                        state = state.setValue(WallBlock.PROPERTY_BY_DIRECTION.get(directions[i]),
                            side == 0 ? WallSide.NONE : side == 1 ? WallSide.LOW : WallSide.TALL);
                    }
                    out.writeBoolean(state.getFluidState().isSource());
                    if (state.useShapeForLightOcclusion()) throw new AssertionError("wall occludes by shape " + state);
                    out.writeBoolean(LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(), state));
                    out.writeBoolean(LightEngine.hasDifferentLightProperties(Blocks.WATER.defaultBlockState(), state));
                    var shapesToTest = new VoxelShape[]{state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO),
                        state.getShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO)};
                    double[][] heights = {{7.5/16, 15.5/16, 23.5/16}, {7.5/16, 14.5/16, 15.5/16}};
                    for (int kind = 0; kind < 2; kind++) for (double y : heights[kind])
                        for (int z = 0; z < 16; z++) {
                            int mask = 0;
                            for (int x = 0; x < 16; x++) {
                                double px = (x + 0.5) / 16.0, pz = (z + 0.5) / 16.0;
                                for (var box : shapesToTest[kind].toAabbs())
                                    if (box.minX < px && box.maxX > px && box.minY < y && box.maxY > y
                                            && box.minZ < pz && box.maxZ > pz) { mask |= 1 << x; break; }
                            }
                            out.writeShort(mask);
                        }
                }
        }
    }
}
