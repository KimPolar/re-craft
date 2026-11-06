import java.lang.reflect.Proxy;
import java.util.List;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.HolderSet;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class JigsawTemplateOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        out.printf("DEFAULT\t%s\t%s%n", Blocks.STRUCTURE_BLOCK.defaultBlockState(), Blocks.JIGSAW.defaultBlockState());
        var template = new StructureTemplate();
        try (var jar = new ZipFile(args[0]); var input = jar.getInputStream(jar.getEntry("data/minecraft/structure/underwater_ruin/warm_1.nbt"))) {
            template.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap()));
        }
        var palettesField = StructureTemplate.class.getDeclaredField("palettes");
        palettesField.setAccessible(true);
        @SuppressWarnings("unchecked") var palettes = (List<StructureTemplate.Palette>) palettesField.get(template);
        var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, arguments) -> {
            if (method.getName().equals("holderLookup")) return BuiltInRegistries.BLOCK;
            if (method.getName().equals("getBlockState")) return ((BlockPos)arguments[0]).getX() % 7 == 0 ? Blocks.BEDROCK.defaultBlockState() : Blocks.STONE.defaultBlockState();
            throw new UnsupportedOperationException(method.toString());
        });
        var origin = new BlockPos(17, 20, -23);
        for (int rotation = 0; rotation < 4; rotation++) for (int clipped = 0; clipped < 2; clipped++) {
            var box = clipped == 0 ? new BoundingBox(-1000, -1000, -1000, 1000, 1000, 1000) : new BoundingBox(14, 20, -26, 19, 23, -20);
            var settings = new StructurePlaceSettings().setRotation(Rotation.values()[rotation]).setBoundingBox(box)
                .addProcessor(BlockIgnoreProcessor.STRUCTURE_BLOCK)
                .addProcessor(JigsawReplacementProcessor.INSTANCE)
                .addProcessor(new BlockRotProcessor(0.83F))
                .addProcessor(new ProtectedBlockProcessor(HolderSet.direct(Blocks.BEDROCK.builtInRegistryHolder())));
            var palette = settings.getRandomPalette(palettes, origin);
            var result = StructureTemplate.processBlockInfos(world, origin, origin, settings, palette.blocks());
            out.printf("CASE\t%d\t%d\t%d%n", rotation, clipped, result.size());
            for (var block : result) out.printf("BLOCK\t%d\t%d\t%d\t%s\t%s%n", block.pos().getX(), block.pos().getY(), block.pos().getZ(), block.state(), block.nbt() == null ? "none" : "present");
        }
    }
}
