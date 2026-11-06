import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.TreeMap;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Vec3i;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.StairBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructurePlaceSettings;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;

public class OceanRuinRegionOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var template = new StructureTemplate();
        var ctor = StructureTemplate.Palette.class.getDeclaredConstructor(List.class); ctor.setAccessible(true);
        var nbt = new CompoundTag(); nbt.putString("LootTable", "minecraft:archaeology/ocean_ruin_warm"); nbt.putLong("LootTableSeed", 1);
        var infos = List.of(
            new StructureTemplate.StructureBlockInfo(new BlockPos(0,0,0), Blocks.STONE_BRICKS.defaultBlockState(), null),
            new StructureTemplate.StructureBlockInfo(new BlockPos(2,0,0), Blocks.SUSPICIOUS_SAND.defaultBlockState(), nbt),
            new StructureTemplate.StructureBlockInfo(new BlockPos(1,0,0), Blocks.STONE_BRICK_STAIRS.defaultBlockState(), null),
            new StructureTemplate.StructureBlockInfo(new BlockPos(1,0,1), Blocks.STONE_BRICK_STAIRS.defaultBlockState().setValue(StairBlock.FACING, Direction.EAST), null));
        var palettes = StructureTemplate.class.getDeclaredField("palettes"); palettes.setAccessible(true);
        ((List<StructureTemplate.Palette>)palettes.get(template)).add(ctor.newInstance(infos));
        var size = StructureTemplate.class.getDeclaredField("size"); size.setAccessible(true); size.set(template, new Vec3i(4,1,2));
        var states = new HashMap<BlockPos, BlockState>();
        var blockTicks = new LinkedHashSet<String>();
        var fluidTicks = new LinkedHashSet<String>();
        RandomSource random = new LegacyRandomSource(42);
        java.util.function.Function<BlockPos,BlockState> get = pos -> states.getOrDefault(pos, pos.getY()<8 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState());
        var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, arguments) -> switch (method.getName()) {
            case "getBlockState" -> get.apply((BlockPos)arguments[0]);
            case "getFluidState" -> get.apply((BlockPos)arguments[0]).getFluidState();
            case "setBlock", "setBlockAndUpdate" -> { states.put(((BlockPos)arguments[0]).immutable(), (BlockState)arguments[1]); yield true; }
            case "scheduleTick" -> {
                var p=(BlockPos)arguments[0]; var target=arguments[1];
                var name=target instanceof Block ? BuiltInRegistries.BLOCK.getKey((Block)target).toString() : BuiltInRegistries.FLUID.getKey((net.minecraft.world.level.material.Fluid)target).toString();
                (target instanceof Block ? blockTicks : fluidTicks).add(name+"\t"+p.getX()+"\t"+p.getY()+"\t"+p.getZ()); yield null;
            }
            case "updateNeighborsAt" -> null;
            case "getBlockEntity" -> null;
            case "isClientSide" -> false;
            case "getRandom" -> random;
            default -> throw new AssertionError("unexpected call " + method);
        });
        var origin = new BlockPos(1,8,1);
        if (!template.placeInWorld(world, origin, BlockPos.ZERO, new StructurePlaceSettings().setIgnoreEntities(true), random, 2)) throw new AssertionError("placement failed");
        for (var state : new BlockState[]{Blocks.STONE_BRICKS.defaultBlockState(), Blocks.STONE_BRICK_STAIRS.defaultBlockState(), Blocks.STONE_BRICK_STAIRS.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,true), Blocks.SUSPICIOUS_SAND.defaultBlockState(), Blocks.BARRIER.defaultBlockState()}) {
            if (state.getPostProcessPos(world,origin) != null) throw new AssertionError("unexpected postprocess state " + state);
        }
        for (var p : List.of(new BlockPos(1,8,1),new BlockPos(2,8,1),new BlockPos(2,8,2),new BlockPos(3,8,1))) {
            var state=get.apply(p); var props=new TreeMap<String,String>();
            state.getValues().forEach(value -> props.put(value.property().getName(), value.valueName()));
            out.println("B\t"+p.getX()+"\t"+p.getY()+"\t"+p.getZ()+"\t"+BuiltInRegistries.BLOCK.getKey(state.getBlock())+"\t"+props);
        }
        for (var tick : blockTicks) out.println("T\t"+tick);
        for (var tick : fluidTicks) out.println("F\t"+tick);
    }
}
