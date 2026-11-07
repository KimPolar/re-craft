import com.mojang.serialization.Lifecycle;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.nbt.*;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePieceSerializationContext;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalPiece;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalStructure;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructureStart;
import net.minecraft.world.level.levelgen.structure.pieces.PiecesContainer;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class RuinedPortalSavedPieceOracle {
    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var manager = (StructureTemplateManager) unsafe.allocateInstance(StructureTemplateManager.class);
        var repository = StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var template = new StructureTemplate();
        var size = StructureTemplate.class.getDeclaredField("size"); size.setAccessible(true);
        size.set(template, new Vec3i(7, 4, 5));
        var id = Identifier.withDefaultNamespace("ruined_portal/portal_1");
        repository.set(manager, new HashMap<>(Map.of(id, Optional.of(template))));
        var blockRegistry = new MappedRegistry<Block>(Registries.BLOCK, Lifecycle.stable());
        blockRegistry.bindTags(Map.of(BlockTags.FEATURES_CANNOT_REPLACE, List.of()));
        blockRegistry.freeze();
        var structureRegistry = new MappedRegistry<Structure>(Registries.STRUCTURE, Lifecycle.stable());
        var structure = (RuinedPortalStructure) unsafe.allocateInstance(RuinedPortalStructure.class);
        Registry.register(structureRegistry, Identifier.withDefaultNamespace("ruined_portal"), structure);
        structureRegistry.freeze();
        var registries = new RegistryAccess.ImmutableRegistryAccess(List.of(blockRegistry, structureRegistry));
        var context = new StructurePieceSerializationContext(null, registries, manager);
        var expected = new ListTag();
        var starts = new ListTag();
        for (var placement : RuinedPortalPiece.VerticalPlacement.values())
            for (var rotation : Rotation.values()) for (var mirror : Mirror.values())
                for (int flags = 0; flags < 64; flags++) {
                    var properties = new RuinedPortalPiece.Properties((flags & 1) != 0, (flags & 32) != 0 ? 0.8f : 0.2f,
                            (flags & 2) != 0, (flags & 4) != 0, (flags & 8) != 0, (flags & 16) != 0);
                    var piece = new RuinedPortalPiece(registries, manager, new BlockPos(-48, 90, 48),
                            placement, properties, id, template, rotation, mirror, new BlockPos(3, 0, 2));
                    var tag = piece.createTag(context);
                    if (starts.isEmpty()) {
                        var chunk = new ChunkPos(-3, 3);
                        starts.add(new StructureStart(structure, chunk, 0, new PiecesContainer(List.of(piece))).createTag(context, chunk));
                    }
                    expected.add(tag);
                    if (!new RuinedPortalPiece(context, tag).createTag(context).equals(tag))
                        throw new AssertionError("Java piece round trip changed: " + tag);
                }
        var root = new CompoundTag(); root.put("pieces", expected); root.put("starts", starts);
        NbtIo.write(root, Path.of(args[0]));
        output.println(expected.get(0));
        if (args.length > 1 && !root.equals(NbtIo.read(Path.of(args[1]))))
            throw new AssertionError("Rust piece NBT differs");
    }
}
