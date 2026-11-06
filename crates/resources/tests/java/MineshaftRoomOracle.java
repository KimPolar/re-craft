import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.util.RandomSource;
import net.minecraft.core.Direction;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePiecesBuilder;
import net.minecraft.world.level.levelgen.structure.structures.MineshaftPieces;
import net.minecraft.world.level.levelgen.structure.structures.MineshaftStructure;

public class MineshaftRoomOracle {
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (long seed : new long[]{Long.MIN_VALUE, -1, 0, 1, 42, 123456789, Long.MAX_VALUE})
            for (int x : new int[]{-1000, -11, -1, 0, 1, 17, 1000})
                for (int z : new int[]{-1000, -11, -1, 0, 1, 17, 1000})
                    for (var variant : MineshaftStructure.Type.values()) {
                        var random = new WorldgenRandom(new LegacyRandomSource(0));
                        random.setLargeFeatureSeed(seed, x, z);
                        random.nextDouble();
                        var room = new MineshaftPieces.MineShaftRoom(0, random, x * 16 + 2, z * 16 + 2, variant);
                        var box = room.getBoundingBox();
                        out.printf("R %d %d %d %s %d %d %d %d %d %d %d%n", seed, x, z,
                            variant.name(), box.minX(), box.minY(), box.minZ(), box.maxX(), box.maxY(), box.maxZ(), random.nextLong());
                    }
        var choose = MineshaftPieces.class.getDeclaredMethod("createRandomShaftPiece",
            StructurePiecesBuilder.class, RandomSource.class, int.class, int.class, int.class,
            Direction.class, int.class, MineshaftStructure.Type.class);
        choose.setAccessible(true);
        for (int seed = -256; seed < 256; seed++)
            for (var direction : new Direction[]{Direction.NORTH, Direction.SOUTH, Direction.WEST, Direction.EAST})
                for (var variant : MineshaftStructure.Type.values()) {
                    var random = new LegacyRandomSource(seed);
                    var piece = choose.invoke(null, new StructurePiecesBuilder(), random,
                        100, 50, 100, direction, 1, variant);
                    if (piece == null) throw new AssertionError("empty region rejected a piece");
                    var prefix = new LegacyRandomSource(seed);
                    prefix.nextInt(100);
                    out.printf("S %d %s %s %s %d%n", seed, direction.getSerializedName(),
                        variant.name(), piece.getClass().getSimpleName(), prefix.nextLong());
                }
        for (int seed = -8; seed < 8; seed++)
            for (var direction : new Direction[]{Direction.NORTH, Direction.SOUTH, Direction.WEST, Direction.EAST})
                for (int x : new int[]{92, 100, 108, 116})
                    for (int z : new int[]{92, 100, 108})
                        for (boolean occupied : new boolean[]{false, true})
                            for (String kind : new String[]{"stairs", "crossing", "corridor"}) {
                                var builder = new StructurePiecesBuilder();
                                var existing = new MineshaftPieces.MineShaftRoom(0, new LegacyRandomSource(7), 100, 100, MineshaftStructure.Type.NORMAL);
                                if (occupied) builder.addPiece(existing);
                                var random = new LegacyRandomSource(seed);
                                var box = switch (kind) {
                                    case "crossing" -> MineshaftPieces.MineShaftCrossing.findCrossing(builder, random, x, 50, z, direction);
                                    case "corridor" -> MineshaftPieces.MineShaftCorridor.findCorridorSize(builder, random, x, 50, z, direction);
                                    default -> MineshaftPieces.MineShaftStairs.findStairs(builder, random, x, 50, z, direction);
                                };
                                var prior = existing.getBoundingBox();
                                out.printf("F %s %d %s %s %d %d %d %d %d %d %d %d %d %s %d %d %d %d %d %d %d%n",
                                    kind, seed, direction.getSerializedName(), occupied,
                                    x, 50, z, prior.minX(), prior.minY(), prior.minZ(), prior.maxX(), prior.maxY(), prior.maxZ(), box != null,
                                    box == null ? 0 : box.minX(), box == null ? 0 : box.minY(), box == null ? 0 : box.minZ(),
                                    box == null ? 0 : box.maxX(), box == null ? 0 : box.maxY(), box == null ? 0 : box.maxZ(), random.nextLong());
                            }
    }
}
