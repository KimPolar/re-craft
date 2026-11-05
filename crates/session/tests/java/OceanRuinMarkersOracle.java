import java.util.ArrayList;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.block.state.properties.StructureMode;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructurePlaceSettings;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;

public class OceanRuinMarkersOracle {
    public static void main(String[] names) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var constructor = StructureTemplate.Palette.class.getDeclaredConstructor(List.class);
        constructor.setAccessible(true);
        var settings = new StructurePlaceSettings();
        for (int size : new int[]{1,2,3,7,16,257}) {
            var palettes = new ArrayList<StructureTemplate.Palette>();
            for (int i=0; i<size; i++) palettes.add(constructor.newInstance(List.of()));
            for (int i=0; i<128; i++) {
                var pos = new BlockPos(i * 173 - 10000, (i * 19) % 511 - 255, 1000 - i * 237);
                int selected = palettes.indexOf(settings.getRandomPalette(palettes, pos));
                output.println("P\t" + size + "\t" + i + "\t" + selected);
            }
        }
        for (int i=0; i<names.length; i++) {
            String name = names[i];
            try (var stream = OceanRuinMarkersOracle.class.getClassLoader().getResourceAsStream("data/minecraft/structure/" + name + ".nbt")) {
                if (stream == null) throw new AssertionError("missing template " + name);
                var template = new StructureTemplate();
                template.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(stream,NbtAccounter.unlimitedHeap()));
                var origin = new BlockPos(i * 7 - 120, 45 + i % 7, 170 - i * 13);
                int radius = (i % 2 == 0) ? 32 : 3;
                var clip = new BoundingBox(origin.getX()-radius,origin.getY()-radius,origin.getZ()-radius,
                        origin.getX()+radius,origin.getY()+radius,origin.getZ()+radius);
                var placement = new StructurePlaceSettings().setRotation(Rotation.values()[i % 4]).setBoundingBox(clip);
                var filtered = template.filterBlocks(origin, placement, Blocks.STRUCTURE_BLOCK);
                var markers = new ArrayList<StructureTemplate.StructureBlockInfo>();
                for (var block : filtered) {
                    var nbt = block.nbt();
                    if (nbt != null && nbt.read("mode", StructureMode.LEGACY_CODEC).orElseThrow() == StructureMode.DATA) markers.add(block);
                }
                var bounds = template.getBoundingBox(placement, origin);
                output.println("T\t"+name+"\t"+markers.size()+"\t"+bounds.minX()+"\t"+bounds.minY()+"\t"+bounds.minZ()+"\t"+bounds.maxX()+"\t"+bounds.maxY()+"\t"+bounds.maxZ());
                for (var marker : markers) {
                    var p = marker.pos();
                    output.println("M\t"+p.getX()+"\t"+p.getY()+"\t"+p.getZ()+"\t"+marker.nbt().getStringOr("metadata",""));
                }
            }
        }
    }
}
