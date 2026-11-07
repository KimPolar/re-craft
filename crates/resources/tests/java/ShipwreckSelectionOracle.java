import java.lang.reflect.Field;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.structure.TemplateStructurePiece;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePiecesBuilder;
import net.minecraft.world.level.levelgen.structure.structures.ShipwreckPieces;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager;

public class ShipwreckSelectionOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var manager = (StructureTemplateManager) unsafe.allocateInstance(StructureTemplateManager.class);
        Field repository = StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        Field sizeField = StructureTemplate.class.getDeclaredField("size"); sizeField.setAccessible(true);
        Field nameField = TemplateStructurePiece.class.getDeclaredField("templateName"); nameField.setAccessible(true);
        var template = new StructureTemplate();
        var templates = new HashMap<Identifier, Optional<StructureTemplate>>();
        for (String name : List.of("with_mast", "upsidedown_full", "upsidedown_fronthalf", "upsidedown_backhalf",
            "sideways_full", "sideways_fronthalf", "sideways_backhalf", "rightsideup_full", "rightsideup_fronthalf", "rightsideup_backhalf",
            "with_mast_degraded", "upsidedown_full_degraded", "upsidedown_fronthalf_degraded", "upsidedown_backhalf_degraded",
            "sideways_full_degraded", "sideways_fronthalf_degraded", "sideways_backhalf_degraded", "rightsideup_full_degraded",
            "rightsideup_fronthalf_degraded", "rightsideup_backhalf_degraded"))
            templates.put(Identifier.withDefaultNamespace("shipwreck/" + name), Optional.of(template));
        repository.set(manager, templates);
        for (int mode=0; mode<2; mode++) for (int seed=0; seed<512; seed++) for (boolean beached : new boolean[]{false,true}) {
            int x=seed*19-4000, z=3000-seed*13;
            int sx=1+seed%41, sy=1+(seed*7)%41, sz=1+(seed*13)%41;
            sizeField.set(template, new Vec3i(sx,sy,sz));
            var random = new WorldgenRandom(new LegacyRandomSource(seed));
            if (mode==1) { random.setLargeFeatureSeed(seed,x,z); x*=16; z*=16; }
            var rotation=Rotation.getRandom(random);
            var piece=ShipwreckPieces.addRandomPiece(manager,new BlockPos(x,90,z),rotation,new StructurePiecesBuilder(),random,beached);
            var box=piece.getBoundingBox();
            int y=beached ? piece.calculateBeachedPosition(-20,random) : 0;
            System.out.printf("C %d %d %s %s %d %d %d %d %d %d %d %s %d %d%n", mode,seed,beached,nameField.get(piece),rotation.ordinal(),
                box.minX(),box.minY(),box.minZ(),box.maxX(),box.maxY(),box.maxZ(),piece.isTooBigToFitInWorldGenRegion(),y,random.nextInt());
        }
    }
}
