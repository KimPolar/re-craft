import java.nio.file.Files;
import java.nio.file.Path;
import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.ScheduledTickAccess;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.LadderBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.material.Fluids;

public class LadderPlacementOracle {
    static final Direction[] HORIZONTAL = {Direction.NORTH, Direction.SOUTH, Direction.WEST, Direction.EAST};
    static class World extends ClientLevel {
        int mask, fluid; boolean adjacentLadder; Direction face;
        World() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public BlockState getBlockState(BlockPos pos) {
            if (adjacentLadder && pos.equals(BlockPos.ZERO.relative(face.getOpposite())))
                return Blocks.LADDER.defaultBlockState().setValue(LadderBlock.FACING, face);
            for (int i = 0; i < HORIZONTAL.length; i++)
                if (pos.equals(BlockPos.ZERO.relative(HORIZONTAL[i])))
                    return ((mask & (1 << i)) != 0 ? Blocks.STONE : Blocks.AIR).defaultBlockState();
            return Blocks.AIR.defaultBlockState();
        }
        @Override public FluidState getFluidState(BlockPos pos) {
            return fluid == 1 ? Fluids.WATER.getSource(false) :
                   fluid == 2 ? Fluids.FLOWING_WATER.getFlowing(4, false) : Fluids.EMPTY.defaultFluidState();
        }
    }
    static class Context extends BlockPlaceContext {
        World world; Direction face; Direction[] directions; boolean replace;
        Context() { super(null, null, null, null, null); }
        @Override public boolean replacingClickedOnBlock() { return replace; }
        @Override public Direction[] getNearestLookingDirections() { return directions; }
        @Override public Direction getClickedFace() { return face; }
        @Override public Level getLevel() { return world; }
        @Override public BlockPos getClickedPos() { return BlockPos.ZERO; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var world = (World) unsafe.allocateInstance(World.class);
        var context = (Context) unsafe.allocateInstance(Context.class); context.world = world;
        for (Direction facing : HORIZONTAL) for (Direction side : Direction.values()) {
            var ladder = Blocks.LADDER.defaultBlockState().setValue(LadderBlock.FACING, facing);
            if (ladder.isFaceSturdy(world, BlockPos.ZERO, side) != (side == facing.getOpposite()))
                throw new AssertionError("unexpected ladder support face " + facing + " " + side);
        }
        int[] scheduled = {0};
        var ticks = (ScheduledTickAccess) java.lang.reflect.Proxy.newProxyInstance(
            ScheduledTickAccess.class.getClassLoader(), new Class<?>[]{ScheduledTickAccess.class},
            (proxy, method, arguments) -> { if (method.getName().equals("scheduleTick")) scheduled[0]++; return null; });
        world.adjacentLadder = false;
        for (Direction facing : HORIZONTAL) for (boolean wet : new boolean[]{false, true})
            for (boolean supported : new boolean[]{false, true}) for (Direction changed : Direction.values()) {
                world.mask = supported ? 15 : 0; scheduled[0] = 0;
                var ladder = Blocks.LADDER.defaultBlockState().setValue(LadderBlock.FACING, facing).setValue(LadderBlock.WATERLOGGED, wet);
                var updated = ladder.updateShape(world, ticks, BlockPos.ZERO, changed, BlockPos.ZERO.relative(changed), Blocks.AIR.defaultBlockState(), net.minecraft.util.RandomSource.create(1));
                boolean removed = changed == facing.getOpposite() && !supported;
                if (updated.isAir() != removed || scheduled[0] != (wet && !removed ? 1 : 0))
                    throw new AssertionError("unexpected ladder update " + facing + " " + wet + " " + supported + " " + changed);
            }
        var rows = new JsonArray();
        for (int shift = 0; shift < 4; shift++)
            for (int mask = 0; mask < 16; mask++)
                for (int fluid = 0; fluid < 3; fluid++)
                    for (boolean replace : new boolean[]{false, true})
                        for (boolean adjacent : new boolean[]{false, true}) {
                            context.face = HORIZONTAL[shift];
                            context.directions = new Direction[]{Direction.UP, HORIZONTAL[shift], HORIZONTAL[(shift+1)%4], Direction.DOWN, HORIZONTAL[(shift+2)%4], HORIZONTAL[(shift+3)%4]};
                            context.replace = replace;
                            world.mask = mask; world.fluid = fluid; world.adjacentLadder = adjacent; world.face = context.face;
                            BlockState placed = Blocks.LADDER.getStateForPlacement(context);
                            var row = new JsonObject(); row.addProperty("shift", shift); row.addProperty("mask", mask);
                            row.addProperty("fluid", fluid); row.addProperty("replace", replace); row.addProperty("adjacent", adjacent);
                            if (placed != null) {
                                row.addProperty("facing", placed.getValue(LadderBlock.FACING).getName());
                                row.addProperty("waterlogged", placed.getValue(LadderBlock.WATERLOGGED));
                            }
                            rows.add(row);
                        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
