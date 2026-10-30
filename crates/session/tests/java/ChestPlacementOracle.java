import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.ChestType;
import net.minecraft.world.level.material.*;

public class ChestPlacementOracle {
    static class World extends ClientLevel {
        Map<BlockPos,BlockState> blocks; boolean water;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState()); }
        @Override public FluidState getFluidState(BlockPos pos) { return water ? Fluids.WATER.defaultFluidState() : Fluids.EMPTY.defaultFluidState(); }
    }
    static class Context extends BlockPlaceContext {
        Level world; Direction face, horizontal; boolean secondary;
        Context() { super(null,null,null,null,null); }
        @Override public Level getLevel() { return world; }
        @Override public BlockPos getClickedPos() { return BlockPos.ZERO; }
        @Override public Direction getClickedFace() { return face; }
        @Override public Direction getHorizontalDirection() { return horizontal; }
        @Override public boolean isSecondaryUseActive() { return secondary; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var ctx=(Context)unsafe.allocateInstance(Context.class); ctx.world=world;
        var horizontal=new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
        var rows=new JsonArray();
        for(boolean trapped : new boolean[]{false,true}) for(var look : horizontal) for(var face : Direction.values())
        for(boolean sneak : new boolean[]{false,true}) for(boolean water : new boolean[]{false,true})
        for(int scenario=0;scenario<34;scenario++) {
            ctx.face=face; ctx.horizontal=look; ctx.secondary=sneak; world.water=water; world.blocks=new HashMap<>();
            var id=trapped ? Blocks.TRAPPED_CHEST : Blocks.CHEST;
            if(scenario>0 && scenario<33) {
                int n=scenario-1; var dir=horizontal[n/8]; var partnerFacing=horizontal[(n%8)/2];
                var partner=id.defaultBlockState().setValue(ChestBlock.FACING,partnerFacing).setValue(ChestBlock.TYPE,(n%2)==0 ? ChestType.SINGLE : ChestType.LEFT);
                world.blocks.put(BlockPos.ZERO.relative(dir),partner);
            } else if(scenario==33) {
                var facing=look.getOpposite(); var neighbor=id.defaultBlockState().setValue(ChestBlock.FACING,facing);
                world.blocks.put(BlockPos.ZERO.relative(facing.getClockWise()),neighbor); world.blocks.put(BlockPos.ZERO.relative(facing.getCounterClockWise()),neighbor);
            }
            var placed=id.getStateForPlacement(ctx);
            var row=new JsonObject(); row.addProperty("trapped",trapped); row.addProperty("look",look.getName()); row.addProperty("face",face.getName()); row.addProperty("sneak",sneak); row.addProperty("water",water);
            var neighbors=new JsonArray(); for(var dir:horizontal) { var block=world.getBlockState(BlockPos.ZERO.relative(dir)); if(!block.isAir()) { var n=new JsonObject(); n.addProperty("dir",dir.getName()); n.addProperty("facing",block.getValue(ChestBlock.FACING).getName()); n.addProperty("type",block.getValue(ChestBlock.TYPE).getSerializedName()); neighbors.add(n); } }
            row.add("neighbors",neighbors); var properties=new JsonObject(); placed.getValues().forEach(v -> properties.addProperty(v.property().getName(),v.valueName())); row.add("properties",properties); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
