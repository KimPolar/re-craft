import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.world.level.block.JigsawBlock;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructurePlaceSettings;

public class JigsawTemplateOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var paletteOrigins = new BlockPos[]{BlockPos.ZERO, new BlockPos(-128, -27, 83),
            new BlockPos(1, 2, 3), new BlockPos(-1, -1, -1),
            new BlockPos(30000000, 319, -30000000), new BlockPos(Integer.MIN_VALUE, 0, Integer.MAX_VALUE)};
        for (int i = 0; i < paletteOrigins.length; i++) {
            out.printf("P\t%d\t%d%n", i, new StructurePlaceSettings().getRandom(paletteOrigins[i]).nextInt(2));
        }
        for (int i = 0; i < args.length; i++) {
            var path = "data/minecraft/structure/" + args[i] + ".nbt";
            try (var stream = JigsawTemplateOracle.class.getClassLoader().getResourceAsStream(path)) {
                if (stream == null) throw new AssertionError("missing " + path);
                var template = new StructureTemplate();
                template.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(stream, NbtAccounter.unlimitedHeap()));
                var origins = new BlockPos[]{BlockPos.ZERO, new BlockPos(-128, -27, 83)};
                for (int r = 0; r < Rotation.values().length; r++) for (int o = 0; o < origins.length; o++) {
                    var list = template.getJigsaws(origins[o], Rotation.values()[r]);
                    out.printf("N\t%d\t%d\t%d\t%d%n", i, r, o, list.size());
                    for (var block : list) {
                        var pos = block.pos();
                        var orientation = block.state().getValue(JigsawBlock.ORIENTATION).getSerializedName();
                        out.printf("J\t%d\t%d\t%d\t%d\t%d\t%d\t%s\t%s\t%s\t%s\t%s\t%d\t%d%n",
                            i, r, o, pos.getX(), pos.getY(), pos.getZ(), orientation,
                            block.jointType().getSerializedName(), block.name(), block.target(),
                            block.pool().identifier(), block.placementPriority(), block.selectionPriority());
                    }
                }
            }
        }
    }
}
