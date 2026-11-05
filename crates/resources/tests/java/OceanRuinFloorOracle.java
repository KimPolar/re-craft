import java.lang.reflect.Proxy;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.FluidTags;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.block.StairBlock;
import net.minecraft.world.level.levelgen.structure.structures.OceanRuinPieces;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.material.Fluids;

public class OceanRuinFloorOracle {
    static int referenceY;
    static int caseNumber;

    static net.minecraft.world.level.block.state.BlockState state(BlockPos p) {
        int bottom = referenceY - Math.floorMod(p.getX() * 31 + p.getZ() * 17 + caseNumber * 7, 13);
        if (p.getY() <= bottom) return Blocks.STONE.defaultBlockState();
        return switch (Math.floorMod(p.getX() * 17 + p.getZ() * 31 + p.getY() + caseNumber, 7)) {
            case 0 -> Blocks.WATER.defaultBlockState();
            case 1 -> Blocks.ICE.defaultBlockState();
            case 2 -> Blocks.OAK_STAIRS.defaultBlockState().setValue(StairBlock.WATERLOGGED, true);
            case 3, 5, 6 -> Blocks.AIR.defaultBlockState();
            default -> Blocks.LAVA.defaultBlockState();
        };
    }

    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", java.util.Collection.class);
        bind.setAccessible(true);
        bind.invoke(Blocks.ICE.builtInRegistryHolder(), List.of(BlockTags.ICE));
        bind.invoke(Fluids.WATER.builtInRegistryHolder(), List.of(FluidTags.WATER));
        bind.invoke(Fluids.FLOWING_WATER.builtInRegistryHolder(), List.of(FluidTags.WATER));
        var unsafeClass = Class.forName("sun.misc.Unsafe");
        var field = unsafeClass.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var pieceClass = Class.forName("net.minecraft.world.level.levelgen.structure.structures.OceanRuinPieces$OceanRuinPiece");
        var piece = unsafeClass.getMethod("allocateInstance", Class.class).invoke(field.get(null), pieceClass);
        var heightMethod = pieceClass.getDeclaredMethod("getHeight", BlockPos.class, BlockGetter.class, BlockPos.class);
        heightMethod.setAccessible(true);
        var world = (BlockGetter) Proxy.newProxyInstance(BlockGetter.class.getClassLoader(), new Class<?>[]{BlockGetter.class}, (proxy, method, values) -> switch (method.getName()) {
            case "getMinY" -> -64;
            case "getBlockState" -> state((BlockPos) values[0]);
            case "getFluidState" -> state((BlockPos) values[0]).getFluidState();
            default -> throw new AssertionError("unexpected " + method);
        });
        int[] heights = {-63, -20, 1, 56, 64, 521, 600};
        for (int i = 0; i < 512; i++) {
            caseNumber = i;
            int initialY = heights[i % heights.length]; referenceY = initialY - 1;
            var start = new BlockPos(-123 + i * 7, initialY, 211 - i * 11);
            var corner = new BlockPos(i % 9, 0, (i / 9) % 9);
            var rotation = Rotation.values()[i % 4];
            var end = StructureTemplate.transform(corner, Mirror.NONE, rotation, BlockPos.ZERO).offset(start);
            int y = (int) heightMethod.invoke(piece, start, world, end);
            output.println("H\t" + i + "\t" + y);
        }
    }
}
