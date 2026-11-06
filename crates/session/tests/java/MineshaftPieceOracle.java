import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePiecesBuilder;
import net.minecraft.world.level.levelgen.structure.structures.MineshaftPieces;
import net.minecraft.world.level.levelgen.structure.structures.MineshaftStructure;

public class MineshaftPieceOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var all = new ListTag();
        for (long seed : new long[]{Long.MIN_VALUE, -1, 0, 1, 42, 123456789, Long.MAX_VALUE})
            for (int[] chunk : new int[][]{{0,0}, {4,-1}, {-11,17}, {1000,-1000}})
                for (var variant : MineshaftStructure.Type.values()) {
                    var random = new WorldgenRandom(new LegacyRandomSource(0));
                    random.setLargeFeatureSeed(seed, chunk[0], chunk[1]);
                    random.nextDouble();
                    var root = new MineshaftPieces.MineShaftRoom(0, random, chunk[0]*16+2, chunk[1]*16+2, variant);
                    var builder = new StructurePiecesBuilder();
                    builder.addPiece(root);
                    root.addChildren(root, builder, random);
                    int offset;
                    if (variant == MineshaftStructure.Type.NORMAL) {
                        offset = builder.moveBelowSeaLevel(63, -64, random, 10);
                    } else {
                        int centerY = builder.getBoundingBox().getCenter().getY();
                        offset = net.minecraft.util.Mth.randomBetweenInclusive(random, 63, 80) - centerY;
                        builder.offsetPiecesVertically(offset);
                    }
                    var pieces = new ListTag();
                    for (var piece : builder.build().pieces()) pieces.add(piece.createTag(null));
                    var graph = new CompoundTag();
                    graph.putLong("seed", seed);
                    graph.putInt("x", chunk[0]);
                    graph.putInt("z", chunk[1]);
                    graph.putString("variant", variant.name());
                    graph.putIntArray("point", new int[]{chunk[0]*16+8, 50+offset, chunk[1]*16});
                    graph.put("pieces", pieces);
                    all.add(graph);
                }
        var result = new CompoundTag();
        result.put("graphs", all);
        NbtIo.write(result, Path.of(args[0]));
    }
}
