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
                    for (int[] heights : new int[][]{{63,-64,50}, {63,-64,63}, {63,-64,128}, {-20,-100,20}, {320,300,321}}) {
                        var placementRandom = new WorldgenRandom(new LegacyRandomSource(0));
                        placementRandom.setLargeFeatureSeed(seed, chunk[0], chunk[1]);
                        placementRandom.nextDouble();
                        var placementRoot = new MineshaftPieces.MineShaftRoom(0, placementRandom, chunk[0]*16+2, chunk[1]*16+2, variant);
                        var placementBuilder = new StructurePiecesBuilder();
                        placementBuilder.addPiece(placementRoot);
                        placementRoot.addChildren(placementRoot, placementBuilder, placementRandom);
                        var center = placementBuilder.getBoundingBox().getCenter();
                        int offset;
                        if (variant == MineshaftStructure.Type.NORMAL) {
                            offset = placementBuilder.moveBelowSeaLevel(heights[0], heights[1], placementRandom, 10);
                        } else {
                            int target = heights[2] <= heights[0] ? heights[0] : net.minecraft.util.Mth.randomBetweenInclusive(placementRandom, heights[0], heights[2]);
                            offset = target - center.getY();
                            placementBuilder.offsetPiecesVertically(offset);
                        }
                        out.printf("H %d %d %d %d,%d,%d %d %d,%d,%d%n", heights[0], heights[1], heights[2],
                            center.getX(), center.getY(), center.getZ(), offset, chunk[0]*16+8, 50+offset, chunk[1]*16);
                        for (var piece : placementBuilder.build().pieces()) out.printf("Q %s%n", box(piece.getBoundingBox()));
                        for (Object entrance : (List<?>)field(placementRoot, "childEntranceBoxes")) out.printf("I %s%n", box((BoundingBox)entrance));
                        out.printf("Y %d%n", placementRandom.nextLong());
                    }
                }
    }
}
