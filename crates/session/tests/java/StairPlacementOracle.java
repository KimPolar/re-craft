import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.material.*;
import net.minecraft.world.phys.Vec3;

public class StairPlacementOracle {
    static final Direction[] CARDINAL={Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
    static class World extends ClientLevel {
        Map<BlockPos,BlockState> blocks; int fluid;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public BlockState getBlockState(BlockPos p) { return blocks.getOrDefault(p,Blocks.AIR.defaultBlockState()); }
        @Override public FluidState getFluidState(BlockPos p) { return switch(fluid) { case 1 -> Fluids.WATER.defaultFluidState(); case 2 -> Fluids.FLOWING_WATER.defaultFluidState(); case 3 -> Fluids.LAVA.defaultFluidState(); default -> Fluids.EMPTY.defaultFluidState(); }; }
    }
    static class Context extends BlockPlaceContext {
        Level world; Direction face, look; double y;
        Context() { super(null,null,null,null,null); }
        @Override public Level getLevel() { return world; }
        @Override public BlockPos getClickedPos() { return BlockPos.ZERO; }
        @Override public Direction getClickedFace() { return face; }
        @Override public Direction getHorizontalDirection() { return look; }
        @Override public Vec3 getClickLocation() { return new Vec3(0.5,y,0.5); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var base=VanillaRegistries.createWorldLookup();
        var reloadable=VanillaRegistries.createReloadableLookup(base);
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) pending.apply();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var ctx=(Context)unsafe.allocateInstance(Context.class); ctx.world=world;
        var looks=new Direction[]{Direction.NORTH,Direction.EAST,Direction.SOUTH,Direction.WEST};
        var heights=new double[]{0.25,0.5,Math.nextUp(0.5),0.75};
        try(var out=new DataOutputStream(Files.newOutputStream(Path.of(args[0])))) {
            for(var look:looks) for(var face:Direction.values()) for(double y:heights) for(int fluid=0;fluid<4;fluid++) for(int scenario=0;scenario<33;scenario++) {
                ctx.face=face; ctx.look=look; ctx.y=y; world.fluid=fluid; world.blocks=new HashMap<>();
                if(scenario>0) { int n=scenario-1; var block=Blocks.STONE_STAIRS.defaultBlockState().setValue(StairBlock.FACING,CARDINAL[(n%8)/2]).setValue(StairBlock.HALF,n%2==0?Half.BOTTOM:Half.TOP); world.blocks.put(BlockPos.ZERO.relative(CARDINAL[n/8]),block); }
                var placed=Blocks.OAK_STAIRS.getStateForPlacement(ctx);
                out.writeByte(placed.getValue(StairBlock.FACING).get3DDataValue());
                out.writeByte(placed.getValue(StairBlock.HALF)==Half.TOP?1:0);
                out.writeByte(placed.getValue(StairBlock.SHAPE).ordinal());
                out.writeByte(placed.getValue(StairBlock.WATERLOGGED)?1:0);
            }
        }
        var items=new ArrayList<String>();
        for(var item:BuiltInRegistries.ITEM) {
            if(item instanceof BlockItem block && block.getBlock() instanceof StairBlock) {
                items.add(BuiltInRegistries.ITEM.getKey(item).toString()+" " + item.getDefaultMaxStackSize());
                for (Direction face:Direction.values()) for(double y:new double[]{0.25,0.75}) for(int fluid=0;fluid<4;fluid++) {
                    ctx.face=face; ctx.look=Direction.NORTH; ctx.y=y; world.fluid=fluid; world.blocks=new HashMap<>();
                    world.blocks.put(BlockPos.ZERO.north(),Blocks.STONE_STAIRS.defaultBlockState().setValue(StairBlock.FACING,Direction.WEST));
                    var expected=Blocks.OAK_STAIRS.getStateForPlacement(ctx);
                    var actual=block.getBlock().getStateForPlacement(ctx);
                    for(var property:new net.minecraft.world.level.block.state.properties.Property[]{StairBlock.FACING,StairBlock.HALF,StairBlock.SHAPE,StairBlock.WATERLOGGED})
                        if(!Objects.equals(expected.getValue(property),actual.getValue(property))) throw new AssertionError(BuiltInRegistries.ITEM.getKey(item)+" "+property);
                }
            }
        }
        Collections.sort(items);
        Files.write(Path.of(args[1]),items);
    }
}
