import com.mojang.serialization.MapCodec;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class OceanRuinFirstPassOracle {
    // The real capped processor requires a world; this preserves its crucial
    // entire-piece behavior without performing archaeology in this test.
    static class EntirePiece implements StructureProcessor {
        public boolean evaluatesEntirePieceState() { return true; }
        public MapCodec<? extends StructureProcessor> codec() { return null; }
    }

    public static void main(String[] names) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var palettesField = StructureTemplate.class.getDeclaredField("palettes");
        palettesField.setAccessible(true);
        var rotations = Rotation.values();
        float[] integrities = {0.5f, 0.7f, 0.8f, 0.9f};
        for (int i = 0; i < names.length; i++) {
            String name = names[i];
            try (var stream = OceanRuinFirstPassOracle.class.getClassLoader()
                    .getResourceAsStream("data/minecraft/structure/" + name + ".nbt")) {
                if (stream == null) throw new AssertionError("missing template " + name);
                var template = new StructureTemplate();
                template.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(stream, NbtAccounter.unlimitedHeap()));
                var palettes = (List<StructureTemplate.Palette>) palettesField.get(template);
                var origin = new BlockPos(-101 + i * 7, 41 + i % 11, 207 - i * 13);
                var settings = new StructurePlaceSettings()
                        .setRotation(rotations[i % 4])
                        .setBoundingBox(new BoundingBox(origin))
                        .addProcessor(new BlockRotProcessor(integrities[(i / 4) % 4]))
                        .addProcessor(BlockIgnoreProcessor.STRUCTURE_AND_AIR)
                        .addProcessor(new EntirePiece());
                var blocks = StructureTemplate.processBlockInfos(null, origin, BlockPos.ZERO, settings, palettes.get(0).blocks());
                output.println("T\t" + name + "\t" + blocks.size());
                for (var block : blocks) {
                    var p = block.pos();
                    output.println("B\t" + p.getX() + "\t" + p.getY() + "\t" + p.getZ() + "\t"
                            + BuiltInRegistries.BLOCK.getKey(block.state().getBlock()) + "\t" + (block.nbt() != null));
                }
            }
        }
    }
}
