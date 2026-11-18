import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructurePiece;
import net.minecraft.world.level.levelgen.structure.StructureStart;
import net.minecraft.world.level.levelgen.structure.pieces.PiecesContainer;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePieceSerializationContext;

public class SavedMineshaftNoopOracle {
    private static class Probe extends StructurePiece {
        int calls;

        Probe(BoundingBox box) {
            super(null, 0, box);
        }

        @Override
        protected void addAdditionalSaveData(StructurePieceSerializationContext context, CompoundTag tag) {}

        @Override
        public void postProcess(WorldGenLevel level, StructureManager structures, ChunkGenerator generator,
                RandomSource random, BoundingBox clip, ChunkPos chunk, BlockPos pivot) {
            calls++;
            random.nextInt();
        }
    }

    public static void main(String[] args) {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        String structureId = args.length == 0 ? "minecraft:mineshaft" : args[0];
        Structure structure = VanillaRegistries.createWorldLookup().lookupOrThrow(Registries.STRUCTURE)
                .getOrThrow(ResourceKey.create(Registries.STRUCTURE, Identifier.parse(structureId)))
                .value();
        int[][] bounds = {
            {0, 0, 32, 7, 7, 39},
            {-1, 0, 47, 6, 7, 54},
            {-1, 1000, 47, 6, 1007, 54},
            {-17, 0, 35, -16, 7, 42},
            {-18, 0, 35, -17, 7, 42}
        };
        ChunkPos target = new ChunkPos(-1, 2);
        BoundingBox clip = new BoundingBox(-16, -63, 32, -1, 319, 47);
        for (int index = 0; index < bounds.length; index++) {
            int[] b = bounds[index];
            Probe piece = new Probe(new BoundingBox(b[0], b[1], b[2], b[3], b[4], b[5]));
            StructureStart start = new StructureStart(structure, new ChunkPos(0, 0), 0,
                    new PiecesContainer(List.of(piece)));
            RandomSource random = RandomSource.create(1234L);
            RandomSource baseline = RandomSource.create(1234L);
            start.placeInChunk(null, null, null, random, clip, target);
            boolean unchanged = random.nextInt() == baseline.nextInt();
            System.out.println("P " + index + " " + piece.calls + " " + unchanged);
        }
    }
}
