import java.lang.reflect.Proxy;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.Fluids;

public class BubbleColumnUpdateOracle {
    static final BlockPos START = new BlockPos(8, 10, 8);
    static BlockState cell(int id) {
        return switch (id) {
            case 0 -> Blocks.AIR.defaultBlockState();
            case 1 -> Blocks.WATER.defaultBlockState();
            case 2 -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 1);
            case 3 -> Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN, true);
            case 4 -> Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN, false);
            case 5 -> Blocks.LAVA.defaultBlockState();
            case 6 -> Blocks.OAK_SLAB.defaultBlockState().setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.WATERLOGGED, true);
            default -> throw new AssertionError();
        };
    }
    static String name(BlockState state) {
        var id=BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString();
        if (state.is(Blocks.BUBBLE_COLUMN)) return id+"/"+state.getValue(BubbleColumnBlock.DRAG_DOWN);
        if (state.is(Blocks.WATER) || state.is(Blocks.LAVA)) return id+"/"+state.getValue(LiquidBlock.LEVEL);
        return id;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var rows=new JsonArray();
        for (int support=0;support<6;support++) for(boolean water:new boolean[]{false,true}) for(boolean lava:new boolean[]{false,true}) {
            boolean down=support==1 || support==3, up=support==2 || support==3;
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(
                BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN, down ? List.of(Blocks.STONE.builtInRegistryHolder()) : List.of(),
                BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP, up ? List.of(Blocks.STONE.builtInRegistryHolder()) : List.of()))).apply();
            var fluids=new ArrayList<Holder<net.minecraft.world.level.material.Fluid>>();
            if (water) fluids.add(Fluids.WATER.builtInRegistryHolder());
            if (lava) fluids.add(Fluids.LAVA.builtInRegistryHolder());
            BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(FluidTags.BUBBLE_COLUMN_CAN_OCCUPY, fluids))).apply();
            for(int a=0;a<7;a++) for(int b=0;b<7;b++) for(int c=0;c<7;c++) {
                var blocks=new HashMap<BlockPos,BlockState>();
                blocks.put(START.below(), support==4?cell(3):support==5?cell(4):Blocks.STONE.defaultBlockState());
                blocks.put(START,cell(a)); blocks.put(START.above(),cell(b)); blocks.put(START.above(2),cell(c));
                var writes=new JsonArray();
                var world=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class<?>[]{LevelAccessor.class},(proxy,method,call)->{
                    if(method.getName().equals("getBlockState")) return blocks.getOrDefault(call[0],Blocks.AIR.defaultBlockState());
                    if(method.getName().equals("setBlock")) {
                        var pos=((BlockPos)call[0]).immutable(); var state=(BlockState)call[1];
                        var changed=blocks.getOrDefault(pos,Blocks.AIR.defaultBlockState())!=state;
                        var event=new JsonArray(); event.add(pos.getY()); event.add(name(state)); event.add(changed); writes.add(event);
                        if(changed) blocks.put(pos,state); return changed;
                    }
                    throw new UnsupportedOperationException(method.toString());
                });
                BubbleColumnBlock.updateColumn(Blocks.BUBBLE_COLUMN,world,START,blocks.get(START.below()));
                var row=new JsonObject(); row.addProperty("support",support); row.addProperty("water",water); row.addProperty("lava",lava);
                row.addProperty("a",a); row.addProperty("b",b); row.addProperty("c",c); row.add("writes",writes); rows.add(row);
            }
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
