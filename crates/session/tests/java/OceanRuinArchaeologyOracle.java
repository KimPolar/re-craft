import java.lang.reflect.Field;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.structures.OceanRuinPieces;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class OceanRuinArchaeologyOracle {
    // The capped processor reads only getLevel().getSeed(); the actual rule's
    // location predicate is AlwaysTrue and does not access the world.
    static final class SeedLevel extends ServerLevel {
        long seed;
        SeedLevel() { super(null, null, null, null, null, null, false, 0L, null, false); }
        @Override public long getSeed() { return seed; }
    }

    public static void main(String[] names) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        Class<?> unsafeClass = Class.forName("sun.misc.Unsafe");
        Field unsafeField = unsafeClass.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        Object unsafe = unsafeField.get(null);
        var level = (SeedLevel) unsafeClass.getMethod("allocateInstance", Class.class).invoke(unsafe, SeedLevel.class);
        var palettesField = StructureTemplate.class.getDeclaredField("palettes");
        palettesField.setAccessible(true);
        Field warm = OceanRuinPieces.class.getDeclaredField("WARM_SUSPICIOUS_BLOCK_PROCESSOR");
        Field cold = OceanRuinPieces.class.getDeclaredField("COLD_SUSPICIOUS_BLOCK_PROCESSOR");
        warm.setAccessible(true);
        cold.setAccessible(true);
        var rotations = Rotation.values();
        float[] integrities = {0.5f, 0.7f, 0.8f, 0.9f};
        for (int i = 0; i < names.length; i++) {
            String name = names[i];
            try (var stream = OceanRuinArchaeologyOracle.class.getClassLoader()
                    .getResourceAsStream("data/minecraft/structure/" + name + ".nbt")) {
                if (stream == null) throw new AssertionError("missing template " + name);
                var template = new StructureTemplate();
                template.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(stream, NbtAccounter.unlimitedHeap()));
                var palettes = (List<StructureTemplate.Palette>) palettesField.get(template);
                var origin = new BlockPos(-101 + i * 7, 41 + i % 11, 207 - i * 13);
                level.seed = ((long) i * 1234567891011L) ^ -982451653L;
                boolean isWarm = name.contains("warm");
                var settings = new StructurePlaceSettings()
                        .setRotation(rotations[i % 4])
                        .setBoundingBox(new BoundingBox(origin))
                        .addProcessor(new BlockRotProcessor(integrities[(i / 4) % 4]))
                        .addProcessor(BlockIgnoreProcessor.STRUCTURE_AND_AIR)
                        .addProcessor((StructureProcessor) (isWarm ? warm : cold).get(null));
                var blocks = StructureTemplate.processBlockInfos(level, origin, BlockPos.ZERO, settings, palettes.get(0).blocks());
                output.println("T\t" + name + "\t" + blocks.size());
                for (var block : blocks) {
                    var p = block.pos();
                    String id = BuiltInRegistries.BLOCK.getKey(block.state().getBlock()).toString();
                    if (id.equals("minecraft:suspicious_sand") || id.equals("minecraft:suspicious_gravel")) {
                        var tag = block.nbt();
                        if (tag == null) throw new AssertionError("missing loot tag");
                        var dusted = block.state().getValues()
                                .filter(v -> v.property().getName().equals("dusted"))
                                .findFirst().orElseThrow().valueName();
                        output.println("A\t" + p.getX() + "\t" + p.getY() + "\t" + p.getZ() + "\t" + id + "\t"
                                + tag.getStringOr("LootTable", "") + "\t" + tag.getLongOr("LootTableSeed", 0L) + "\t" + dusted);
                    } else {
                        output.println("B\t" + p.getX() + "\t" + p.getY() + "\t" + p.getZ() + "\t" + id + "\t" + (block.nbt() != null));
                    }
                }
            }
        }
    }
}
