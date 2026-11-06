import java.lang.reflect.Field;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Optional;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Vec3i;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.structures.OceanRuinPieces.OceanRuinPiece;
import net.minecraft.world.level.levelgen.structure.structures.OceanRuinStructure;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager;

public class OceanRuinSavedPieceOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var manager = (StructureTemplateManager) unsafe.allocateInstance(StructureTemplateManager.class);
        var repository = StructureTemplateManager.class.getDeclaredField("structureRepository");
        repository.setAccessible(true);
        var template = new StructureTemplate();
        var size = StructureTemplate.class.getDeclaredField("size");
        size.setAccessible(true);
        size.set(template, new Vec3i(7, 4, 5));
        var id = Identifier.withDefaultNamespace("underwater_ruin/warm_1");
        var templates = new HashMap<Identifier, Optional<StructureTemplate>>();
        templates.put(id, Optional.of(template));
        repository.set(manager, templates);
        var expected = new ListTag();
        for (var type : OceanRuinStructure.Type.values())
            for (var rotation : Rotation.values())
                for (boolean large : new boolean[]{false, true})
                    for (boolean moved : new boolean[]{false, true}) {
                        var piece = new OceanRuinPiece(manager, id, new BlockPos(-37, 90, 52), rotation, 0.7f, type, large);
                        if (moved) piece.move(0, -37, 0);
                        var tag = piece.createTag(null);
                        expected.add(tag);
                        if (!OceanRuinPiece.create(manager, tag).createTag(null).equals(tag))
                            throw new AssertionError("Java round trip changed: " + tag);
                    }
        var root = new CompoundTag();
        root.put("pieces", expected);
        NbtIo.write(root, Path.of(args[0]));
        System.out.println(expected.get(0));
        if (args.length > 1) {
            var actual = NbtIo.read(Path.of(args[1]));
            if (!root.equals(actual)) throw new AssertionError("Rust NBT differs: expected=" + root + " actual=" + actual);
        }
    }
}
