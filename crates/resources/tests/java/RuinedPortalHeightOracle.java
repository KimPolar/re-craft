import com.mojang.serialization.MapCodec;
import java.util.*;
import java.util.concurrent.CompletableFuture;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalPiece;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalStructure;

public class RuinedPortalHeightOracle {
    static class Generator extends ChunkGenerator {
        Generator(BiomeSource source) { super(source); }
        protected MapCodec<? extends ChunkGenerator> codec() { throw new UnsupportedOperationException(); }
        public void spawnOriginalMobs(WorldGenRegion region) {}
        public int getGenDepth() { return 384; }
        public int getSeaLevel() { return 63; }
        public int getMinY() { return -64; }
        public int getBaseHeight(int x, int z, Heightmap.Types map, LevelHeightAccessor a, RandomState r) { throw new UnsupportedOperationException(); }
        public NoiseColumn getBaseColumn(int x, int z, LevelHeightAccessor accessor, RandomState random) {
            var blocks = new BlockState[384];
            int floor = 60 + Math.floorMod(x * 13 + z * 7, 11) - 5;
            for (int i = 0; i < blocks.length; i++) {
                int y = i - 64;
                blocks[i] = y <= floor ? Blocks.STONE.defaultBlockState() :
                        y <= floor + 3 ? Blocks.WATER.defaultBlockState() : Blocks.AIR.defaultBlockState();
            }
            return new NoiseColumn(-64, blocks);
        }
        public void addDebugScreenInfo(List<String> lines, RandomState random, BlockPos pos, SamplerContext context) {}
        public CompletableFuture<ChunkAccess> buildTerrain(ChunkAccess chunk, Blender blender, RandomState random,
                StructureManager structures, BiomeManager biomes, WorldGenRegion world, Set<Holder<Biome>> possible) {
            throw new UnsupportedOperationException();
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        bind.setAccessible(true);
        bind.invoke(Blocks.STONE.builtInRegistryHolder(), List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP));
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var biome = Holder.direct((Biome) unsafe.allocateInstance(Biome.class));
        var generator = new Generator(new FixedBiomeSource(biome));
        var method = RuinedPortalStructure.class.getDeclaredMethod("findSuitableY", RandomSource.class,
                ChunkGenerator.class, RuinedPortalPiece.VerticalPlacement.class, boolean.class, int.class,
                int.class, BoundingBox.class, LevelHeightAccessor.class, RandomState.class);
        method.setAccessible(true);
        long[] seeds = {0, 1, -1, 42, 123456789, Long.MIN_VALUE, Long.MAX_VALUE};
        int[] surfaces = {-60, 40, 70, 100};
        for (var placement : RuinedPortalPiece.VerticalPlacement.values()) for (boolean pocket : new boolean[]{false,true})
            for (long seed : seeds) for (int surface : surfaces) for (int i = 0; i < 7; i++) {
                int x = i * 31 - 90, z = 70 - i * 23, height = i + 5;
                var bounds = new BoundingBox(x, 0, z, x + 7 + i, height - 1, z + 9);
                var random = new LegacyRandomSource(seed);
                int y = (int) method.invoke(null, random, generator, placement, pocket, surface, height,
                        bounds, LevelHeightAccessor.create(-64, 384), null);
                System.out.printf("H %s %s %d %d %d %d %d %d %d%n", placement.name(), pocket, seed,
                        surface, x, z, height, y, random.nextInt());
            }
    }
}
