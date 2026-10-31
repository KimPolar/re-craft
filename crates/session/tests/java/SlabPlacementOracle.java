import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.world.item.*;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.material.*;
import net.minecraft.world.phys.Vec3;

public class SlabPlacementOracle {
    static class World extends ClientLevel {
        BlockState block; int fluid;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public BlockState getBlockState(BlockPos p) { return block; }
        @Override public FluidState getFluidState(BlockPos p) { return switch(fluid) { case 1 -> Fluids.WATER.defaultFluidState(); case 2 -> Fluids.FLOWING_WATER.defaultFluidState(); case 3 -> Fluids.LAVA.defaultFluidState(); default -> Fluids.EMPTY.defaultFluidState(); }; }
    }
    static class Context extends BlockPlaceContext {
        Level world; Direction face; double y; boolean replacing; ItemStack stack;
        Context() { super(null,null,null,null,null); }
        @Override public Level getLevel() { return world; }
        @Override public BlockPos getClickedPos() { return BlockPos.ZERO; }
        @Override public Direction getClickedFace() { return face; }
        @Override public Vec3 getClickLocation() { return new Vec3(0.5,y,0.5); }
        @Override public boolean replacingClickedOnBlock() { return replacing; }
        @Override public ItemStack getItemInHand() { return stack; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var base=VanillaRegistries.createWorldLookup();
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(VanillaRegistries.createReloadableLookup(base))) pending.apply();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var ctx=(Context)unsafe.allocateInstance(Context.class); ctx.world=world;
        if(!Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.DOUBLE).setValue(SlabBlock.WATERLOGGED,true).getFluidState().is(Fluids.WATER)) throw new AssertionError("waterlogged double slab fluid");
        var heights=new double[]{0.25,0.5,Math.nextUp(0.5),0.75};
        try(var out=new DataOutputStream(Files.newOutputStream(Path.of(args[0])))) {
            for(int initial=0;initial<5;initial++) for(boolean sameItem:new boolean[]{false,true}) for(boolean replacing:new boolean[]{false,true})
                for(var face:Direction.values()) for(double y:heights) for(int fluid=0;fluid<4;fluid++) {
                    world.block=switch(initial) {
                        case 0 -> Blocks.AIR.defaultBlockState();
                        case 1 -> Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.BOTTOM);
                        case 2 -> Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.TOP);
                        case 3 -> Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.DOUBLE);
                        default -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.BOTTOM);
                    };
                    ctx.face=face; ctx.y=y; ctx.replacing=replacing; ctx.stack=new ItemStack(sameItem?Items.OAK_SLAB:Items.STONE_SLAB); world.fluid=fluid;
                    out.writeBoolean(world.block.canBeReplaced(ctx));
                    var placed=Blocks.OAK_SLAB.getStateForPlacement(ctx);
                    out.writeByte(placed.getValue(SlabBlock.TYPE).ordinal());
                    out.writeBoolean(placed.getValue(SlabBlock.WATERLOGGED));
                }
        }
        var items=new ArrayList<String>();
        world.block=Blocks.AIR.defaultBlockState(); ctx.stack=new ItemStack(Items.OAK_SLAB);
        for(var item:BuiltInRegistries.ITEM) if(item instanceof BlockItem block && block.getBlock() instanceof SlabBlock) {
            items.add(BuiltInRegistries.ITEM.getKey(item).toString()+" "+item.getDefaultMaxStackSize());
            for(var face:Direction.values()) for(double y:heights) for(int fluid=0;fluid<4;fluid++) {
                ctx.face=face; ctx.y=y; world.fluid=fluid;
                var expected=Blocks.OAK_SLAB.getStateForPlacement(ctx);
                var actual=block.getBlock().getStateForPlacement(ctx);
                if(!Objects.equals(expected.getValue(SlabBlock.TYPE),actual.getValue(SlabBlock.TYPE)) || !Objects.equals(expected.getValue(SlabBlock.WATERLOGGED),actual.getValue(SlabBlock.WATERLOGGED))) throw new AssertionError(BuiltInRegistries.ITEM.getKey(item));
            }
        }
        Collections.sort(items); Files.write(Path.of(args[1]),items);
    }
}
