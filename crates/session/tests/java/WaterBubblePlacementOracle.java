import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.redstone.CollectingNeighborUpdater;

public class WaterBubblePlacementOracle {
    static final BlockPos CENTER=new BlockPos(8,10,8);
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(IceBubbleTagOracle.World)unsafe.allocateInstance(IceBubbleTagOracle.World.class);
        world.attributes=EnvironmentAttributeSystem.builder().build();
        var rows=new JsonArray();
        for(int tags=0;tags<4;tags++) for(boolean waterTagged:new boolean[]{false,true}) for(int scenario=0;scenario<9;scenario++) {
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,Map.of(
                BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,(tags & 1)!=0?List.of(Blocks.STONE.builtInRegistryHolder()):List.of(),
                BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP,(tags & 2)!=0?List.of(Blocks.STONE.builtInRegistryHolder()):List.of()))).apply();
            BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID,Map.of(
                FluidTags.WATER,List.of(Fluids.WATER.builtInRegistryHolder(),Fluids.FLOWING_WATER.builtInRegistryHolder()),
                FluidTags.BUBBLE_COLUMN_CAN_OCCUPY,waterTagged?List.of(Fluids.WATER.builtInRegistryHolder()):List.of()))).apply();
            world.blocks=new HashMap<>(); world.random=new LegacyRandomSource(1); world.bubble=false; world.updater=new CollectingNeighborUpdater(world,100000); world.blockTicks=new JsonArray(); world.fluidTicks=new JsonArray(); world.pending=new HashSet<>();
            var water=Blocks.WATER.defaultBlockState(); var flowing=water.setValue(LiquidBlock.LEVEL,1); var stone=Blocks.STONE.defaultBlockState();
            BlockPos target;
            if(scenario==0 || scenario==4) { target=CENTER; world.blocks.put(CENTER.below(),stone); if(scenario==4) world.blocks.put(CENTER,flowing); }
            else if(scenario==1) { target=CENTER.below(); world.blocks.put(CENTER,water); }
            else if(scenario==5) { target=CENTER; for(var side:new Direction[]{Direction.WEST,Direction.EAST}) { var pos=CENTER.relative(side); world.blocks.put(pos,water); world.blocks.put(pos.below(),stone); } }
            else if(scenario>=6) { world.blocks.put(CENTER,Blocks.BUBBLE_COLUMN.defaultBlockState()); if(scenario!=8) world.blocks.put(CENTER.below(),stone); target=scenario==6?CENTER.below():scenario==7?CENTER.above():CENTER.east(); }
            else { target=CENTER.east(); world.blocks.put(CENTER.below(),stone); world.blocks.put(CENTER,scenario==3?flowing:water); }
            world.setBlock(target,scenario==6?Blocks.AIR.defaultBlockState():(scenario==0||scenario==4||scenario==7)?water:stone,3,512);
            var row=new JsonObject(); row.addProperty("tags",tags); row.addProperty("water",waterTagged); row.addProperty("scenario",scenario);
            row.add("blockTicks",world.blockTicks); row.add("fluidTicks",world.fluidTicks); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
