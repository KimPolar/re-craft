import java.lang.reflect.Proxy;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.HolderSet;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate.StructureBlockInfo;

public class StructureProcessorsOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var rot = new BlockRotProcessor(HolderSet.direct(Blocks.DEEPSLATE_BRICKS.builtInRegistryHolder(), Blocks.DEEPSLATE_TILES.builtInRegistryHolder()), 0.95F);
        var rule = new RuleProcessor(List.of(
            new ProcessorRule(new RandomBlockMatchTest(Blocks.DEEPSLATE_BRICKS, 0.3F), AlwaysTrueTest.INSTANCE, Blocks.CRACKED_DEEPSLATE_BRICKS.defaultBlockState()),
            new ProcessorRule(new RandomBlockMatchTest(Blocks.DEEPSLATE_TILES, 0.3F), AlwaysTrueTest.INSTANCE, Blocks.CRACKED_DEEPSLATE_TILES.defaultBlockState()),
            new ProcessorRule(new RandomBlockMatchTest(Blocks.STONE, 0.6F), new RandomBlockMatchTest(Blocks.DIRT, 0.5F), Blocks.AIR.defaultBlockState()),
            new ProcessorRule(new RandomBlockMatchTest(Blocks.STONE, 0.7F), AlwaysTrueTest.INSTANCE, Blocks.DIRT.defaultBlockState())
        ));
        var protect = new ProtectedBlockProcessor(HolderSet.direct(Blocks.BEDROCK.builtInRegistryHolder()));
        var input = List.of(Blocks.DEEPSLATE_BRICKS, Blocks.DEEPSLATE_TILES, Blocks.STONE, Blocks.OAK_PLANKS);
        var existing = List.of(Blocks.DIRT, Blocks.BEDROCK);
        var settings = new StructurePlaceSettings();
        for (int x = -8; x <= 8; x++) for (int y : new int[]{-64, 0, 63, 319}) for (int i = 0; i < input.size(); i++) for (int w = 0; w < existing.size(); w++) {
            var existingState = existing.get(w).defaultBlockState();
            var world = (LevelReader) Proxy.newProxyInstance(LevelReader.class.getClassLoader(), new Class<?>[]{LevelReader.class}, (proxy, method, arguments) -> {
                if (method.getName().equals("getBlockState")) return existingState;
                throw new UnsupportedOperationException(method.toString());
            });
            var pos = new BlockPos(x, y, x * 13 - 7);
            var block = new StructureBlockInfo(pos, input.get(i).defaultBlockState(), null);
            for (var processor : List.of(rot, rule, protect)) {
                if (block == null) break;
                block = processor.processBlock(world, pos, pos, pos, block, settings);
            }
            out.printf("RESULT\t%d\t%d\t%d\t%d\t%s%n", x, y, i, w, block == null ? "<skip>" : block.state());
        }
    }
}
