import java.lang.reflect.Proxy;
import java.util.List;
import java.util.ArrayList;
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
import net.minecraft.world.level.levelgen.Heightmap;
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
        var writes = new ArrayList<String>();
        var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(), new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, arguments) -> {
            if (method.getName().equals("holderLookup")) return BuiltInRegistries.BLOCK;
            if (method.getName().equals("getBlockState")) return ((BlockPos)arguments[0]).getX() % 7 == 0 ? Blocks.BEDROCK.defaultBlockState() : Blocks.STONE.defaultBlockState();
            if (method.getName().equals("getHeight")) {
                if (arguments[0] != Heightmap.Types.WORLD_SURFACE_WG) throw new IllegalStateException("unexpected heightmap");
                return 21 + Math.floorMod((int)arguments[1] + (int)arguments[2], 5);
            }
            if (method.getName().equals("setBlock")) {
                var pos = (BlockPos)arguments[0];
                writes.add(String.format("WRITE\t%d\t%d\t%d\t%s\t%d", pos.getX(), pos.getY(), pos.getZ(), arguments[1], arguments[2]));
                return true;
            }
            throw new UnsupportedOperationException(method.toString());
        });
        var origin = new BlockPos(17, 20, -23);
        for (int projection = 0; projection < 2; projection++) for (int rotation = 0; rotation < 4; rotation++) for (int clipped = 0; clipped < 2; clipped++) {
            var box = clipped == 0 ? new BoundingBox(-1000, -1000, -1000, 1000, 1000, 1000) : new BoundingBox(14, 20, -26, 19, 23, -20);
            var settings = new StructurePlaceSettings().setRotation(Rotation.values()[rotation]).setBoundingBox(box)
                .setKnownShape(true).setIgnoreEntities(true).setLiquidSettings(LiquidSettings.IGNORE_WATERLOGGING)
                .addProcessor(BlockIgnoreProcessor.STRUCTURE_BLOCK)
                .addProcessor(JigsawReplacementProcessor.INSTANCE)
                .addProcessor(new BlockRotProcessor(0.83F))
                .addProcessor(new ProtectedBlockProcessor(HolderSet.direct(Blocks.BEDROCK.builtInRegistryHolder())));
            if (projection == 1) settings.addProcessor(new GravityProcessor(Heightmap.Types.WORLD_SURFACE_WG, -1));
            var palette = settings.getRandomPalette(palettes, origin);
            var result = StructureTemplate.processBlockInfos(world, origin, origin, settings, palette.blocks());
            out.printf("CASE\t%d\t%d\t%d\t%d%n", projection, rotation, clipped, result.size());
            for (var block : result) out.printf("BLOCK\t%d\t%d\t%d\t%s\t%s%n", block.pos().getX(), block.pos().getY(), block.pos().getZ(), block.state(), block.nbt() == null ? "none" : "present");
            writes.clear();
            var placed = template.placeInWorld(world, origin, origin, settings, new net.minecraft.world.level.levelgen.LegacyRandomSource(77L), 18);
            out.printf("PLACE\t%s\t%d%n", placed, writes.size());
            for (var write : writes) out.println(write);
        }
    }
}
