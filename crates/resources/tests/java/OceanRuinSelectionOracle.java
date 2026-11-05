import java.lang.reflect.Field;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.TemplateStructurePiece;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePiecesBuilder;
import net.minecraft.world.level.levelgen.structure.structures.OceanRuinPieces;
import net.minecraft.world.level.levelgen.structure.structures.OceanRuinStructure;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager;

public class OceanRuinSelectionOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var manager = (StructureTemplateManager) unsafe.allocateInstance(StructureTemplateManager.class);
        Field repository = StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var templates = new HashMap<Identifier, Optional<StructureTemplate>>();
        for (var prefix : List.of("brick", "cracked", "mossy", "warm"))
            for (int i = 1; i <= 8; i++) templates.put(Identifier.withDefaultNamespace("underwater_ruin/"+prefix+"_"+i), Optional.of(new StructureTemplate()));
        for (var prefix : List.of("big_brick", "big_cracked", "big_mossy"))
            for (int i : new int[]{1,2,3,8}) templates.put(Identifier.withDefaultNamespace("underwater_ruin/"+prefix+"_"+i), Optional.of(new StructureTemplate()));
        for (int i = 4; i <= 7; i++) templates.put(Identifier.withDefaultNamespace("underwater_ruin/big_warm_"+i), Optional.of(new StructureTemplate()));
        repository.set(manager, templates);
        var biome = Holder.direct((Biome) unsafe.allocateInstance(Biome.class));
        var settings = new Structure.StructureSettings(HolderSet.direct(biome));
        Field name = TemplateStructurePiece.class.getDeclaredField("templateName"); name.setAccessible(true);
        Field integrity = OceanRuinPieces.OceanRuinPiece.class.getDeclaredField("integrity"); integrity.setAccessible(true);
        Field large = OceanRuinPieces.OceanRuinPiece.class.getDeclaredField("isLarge"); large.setAccessible(true);
        float[] probabilities = {0.0f, 0.35f, 1.0f};
        int id = 0;
        for (int mode = 0; mode < 2; mode++) for (int seed = 0; seed < 128; seed++) for (var type : OceanRuinStructure.Type.values())
            for (float big : probabilities) for (float cluster : probabilities) {
                int x = seed * 19 - 1100, z = 701 - seed * 13;
                var random = new WorldgenRandom(new LegacyRandomSource(seed));
                if (mode == 1) { random.setLargeFeatureSeed(seed, x, z); x *= 16; z *= 16; }
                var rotation = Rotation.getRandom(random);
                var builder = new StructurePiecesBuilder();
                OceanRuinPieces.addPieces(manager, new BlockPos(x,90,z), rotation, builder, random,
                    new OceanRuinStructure(settings,type,big,cluster));
                var pieces = builder.build().pieces();
                System.out.printf("C %d %d %d %s %s %s %d %d%n", id, mode, seed, type.name(), big, cluster, pieces.size(), random.nextInt());
                for (var piece : pieces) {
                    var template = (TemplateStructurePiece) piece;
                    var pos = template.templatePosition();
                    System.out.printf("P %s %d %d %d %d %d %s%n", name.get(template), pos.getX(), pos.getY(), pos.getZ(),
                        template.getRotation().ordinal(), Float.floatToIntBits(integrity.getFloat(piece)), large.getBoolean(piece));
                }
                id++;
            }
    }
}
