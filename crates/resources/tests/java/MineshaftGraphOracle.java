import java.lang.reflect.Field;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePiecesBuilder;
import net.minecraft.world.level.levelgen.structure.structures.MineshaftPieces;
import net.minecraft.world.level.levelgen.structure.structures.MineshaftStructure;

public class MineshaftGraphOracle {
    private static Object field(Object value, String name) throws Exception {
        Field field = value.getClass().getDeclaredField(name);
        field.setAccessible(true);
        return field.get(value);
    }
    private static String box(BoundingBox b) {
        return b.minX()+","+b.minY()+","+b.minZ()+","+b.maxX()+","+b.maxY()+","+b.maxZ();
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var out = System.out;
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
                    out.printf("G %d %d %d %s%n", seed, chunk[0], chunk[1], variant.name());
                    for (var piece : builder.build().pieces()) {
                        String kind = piece.getClass().getSimpleName();
                        var orientation = piece.getOrientation();
                        if (kind.equals("MineShaftCrossing")) orientation = (net.minecraft.core.Direction)field(piece, "direction");
                        String extra = switch (kind) {
                            case "MineShaftRoom" -> "room";
                            case "MineShaftStairs" -> "stairs";
                            case "MineShaftCrossing" -> "crossing," + field(piece, "isTwoFloored");
                            case "MineShaftCorridor" -> "corridor," + field(piece, "hasRails") + "," + field(piece, "spiderCorridor") + "," + field(piece, "numSections");
                            default -> throw new AssertionError(kind);
                        };
                        out.printf("P %s %d %s %s%n", orientation == null ? "none" : orientation.getSerializedName(), piece.getGenDepth(), box(piece.getBoundingBox()), extra);
                    }
                    for (Object entrance : (List<?>)field(root, "childEntranceBoxes")) out.printf("E %s%n", box((BoundingBox)entrance));
                    out.printf("Z %d%n", random.nextLong());
                }
    }
}
