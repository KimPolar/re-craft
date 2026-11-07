import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.resources.Identifier;
import net.minecraft.util.Util;
import net.minecraft.util.random.WeightedList;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.TemplateFeature;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructurePlaceSettings;
import java.util.List;

public class TemplateFeatureOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var first = new TemplateFeature.TemplateEntry(Identifier.parse("test:first"), List.of(Rotation.values()));
        var second = new TemplateFeature.TemplateEntry(Identifier.parse("test:second"), List.of(Rotation.CLOCKWISE_90, Rotation.NONE));
        var choices = WeightedList.<TemplateFeature.TemplateEntry>builder().add(first, 3).add(second, 7).build();
        var feature = new TemplateFeature(choices);
        var offset = TemplateFeature.class.getDeclaredMethod("getRotatedOffset", Rotation.class, Direction.Axis.class, StructureTemplate.class);
        offset.setAccessible(true);
        var sizeField = StructureTemplate.class.getDeclaredField("size"); sizeField.setAccessible(true);
        var paletteClass = Class.forName("net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate$Palette");
        var paletteConstructor = paletteClass.getDeclaredConstructor(List.class); paletteConstructor.setAccessible(true);
        var palettes = new java.util.ArrayList<StructureTemplate.Palette>();
        for (int i=0;i<3;i++) palettes.add((StructureTemplate.Palette)paletteConstructor.newInstance(List.of()));
        for (long seed=0; seed<128; seed++) {
            var random = new WorldgenRandom(new XoroshiroRandomSource(seed));
            var selected = choices.getRandomOrThrow(random);
            var rotation = Util.getRandom(selected.rotations(), random);
            var template = new StructureTemplate();
            sizeField.set(template, selected == first ? new Vec3i(7, 4, 5) : new Vec3i(8, 3, 6));
            var x = (Vec3i) offset.invoke(feature, rotation, Direction.Axis.X, template);
            var z = (Vec3i) offset.invoke(feature, rotation, Direction.Axis.Z, template);
            var origin = new BlockPos(-48, 70, 39).offset(x).offset(z);
            var palette = new StructurePlaceSettings().setRandom(random).getRandomPalette(palettes, origin);
            System.out.printf("T %d %s %s %d %d %d %d %d%n", seed, selected.template(), rotation.name(), origin.getX(), origin.getY(), origin.getZ(), palettes.indexOf(palette), random.nextLong());
        }
    }
}
