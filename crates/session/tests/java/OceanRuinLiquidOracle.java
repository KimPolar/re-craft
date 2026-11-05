import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Vec3i;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructurePlaceSettings;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;

public class OceanRuinLiquidOracle {
    static void run(boolean templateWater) throws Exception {
        var template = new StructureTemplate();
        var paletteConstructor = StructureTemplate.Palette.class.getDeclaredConstructor(List.class);
        paletteConstructor.setAccessible(true);
        var stair = Blocks.STONE_BRICK_STAIRS.defaultBlockState();
        var infos = new ArrayList<StructureTemplate.StructureBlockInfo>();
        for (int x = 2; x >= 0; x--) infos.add(new StructureTemplate.StructureBlockInfo(new BlockPos(x,0,0), stair, null));
        if (templateWater) infos.add(new StructureTemplate.StructureBlockInfo(new BlockPos(-1,0,0), Blocks.WATER.defaultBlockState(), null));
        var palettes = StructureTemplate.class.getDeclaredField("palettes"); palettes.setAccessible(true);
        ((List<StructureTemplate.Palette>)palettes.get(template)).add(paletteConstructor.newInstance(infos));
        var size = StructureTemplate.class.getDeclaredField("size"); size.setAccessible(true); size.set(template, new Vec3i(4,1,1));
        var blocks = new HashMap<BlockPos, BlockState>();
        if (!templateWater) {
            blocks.put(new BlockPos(-1,0,0), Blocks.WATER.defaultBlockState());
            for (int x = 0; x <= 2; x++) blocks.put(new BlockPos(x,0,0), Blocks.WATER.defaultBlockState().setValue(BlockStateProperties.LEVEL,1));
        } else {
            blocks.put(new BlockPos(0,-1,0), Blocks.WATER.defaultBlockState());
        }
        var waterlogged = new ArrayList<Integer>();
        var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, args) -> switch (method.getName()) {
            case "getBlockState" -> blocks.getOrDefault((BlockPos)args[0], Blocks.AIR.defaultBlockState());
            case "getFluidState" -> blocks.getOrDefault((BlockPos)args[0], Blocks.AIR.defaultBlockState()).getFluidState();
            case "setBlock", "setBlockAndUpdate" -> {
                var pos = (BlockPos)args[0]; var state = (BlockState)args[1]; blocks.put(pos.immutable(), state);
                if (method.getName().equals("setBlockAndUpdate") && state.hasProperty(BlockStateProperties.WATERLOGGED) && state.getValue(BlockStateProperties.WATERLOGGED)) waterlogged.add(pos.getX());
                yield true;
            }
            case "scheduleTick", "updateNeighborsAt" -> null;
            case "isClientSide" -> false;
            case "getBlockEntity" -> null;
            default -> throw new AssertionError("unexpected call " + method);
        });
        var settings = new StructurePlaceSettings().setKnownShape(true).setIgnoreEntities(true);
        if (!template.placeInWorld(world, BlockPos.ZERO, BlockPos.ZERO, settings, new LegacyRandomSource(1), 2)) throw new AssertionError("placement failed");
        var expected = templateWater ? List.<Integer>of() : List.of(0,1,2);
        if (!waterlogged.equals(expected)) throw new AssertionError("water order " + waterlogged + " expected " + expected);
        for (int x = 0; x <= 2; x++) {
            boolean wet = blocks.get(new BlockPos(x,0,0)).getValue(BlockStateProperties.WATERLOGGED);
            if (wet == templateWater) throw new AssertionError("wrong water state " + x);
        }
    }
    public static void main(String[] args) throws Exception {
        var output = System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        run(false); run(true); output.println("OK");
    }
}
