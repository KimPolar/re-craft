import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import com.mojang.serialization.MapCodec;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.NoiseColumn;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.feature.Feature;
import net.minecraft.world.level.levelgen.feature.SimpleBlockFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.levelgen.placement.CountPlacement;
import net.minecraft.world.level.levelgen.placement.InSquarePlacement;
import net.minecraft.world.level.levelgen.placement.PlacedFeature;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.pools.FeaturePoolElement;
import net.minecraft.world.level.levelgen.structure.pools.StructureTemplatePool;
import net.minecraft.world.level.levelgen.structure.templatesystem.LiquidSettings;

public class JigsawFeatureOracle {
    static class Generator extends ChunkGenerator {
        Generator() { super(null); }
        protected MapCodec<? extends ChunkGenerator> codec() { throw new UnsupportedOperationException(); }
        public void spawnOriginalMobs(WorldGenRegion region) {}
        public int getGenDepth() { return 16; }
        public int getSeaLevel() { return 8; }
        public int getMinY() { return 0; }
        public int getBaseHeight(int x, int z, Heightmap.Types type, LevelHeightAccessor accessor, RandomState state) { return 0; }
        public NoiseColumn getBaseColumn(int x, int z, LevelHeightAccessor accessor, RandomState state) { throw new UnsupportedOperationException(); }
        public void addDebugScreenInfo(List<String> list, RandomState state, BlockPos pos, SamplerContext context) {}
        public CompletableFuture<ChunkAccess> buildTerrain(ChunkAccess chunk, Blender blender, RandomState state, StructureManager structures, BiomeManager biomes, WorldGenRegion world, Set<Holder<Biome>> biomeSet) { throw new UnsupportedOperationException(); }
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        Holder<Feature> feature = Holder.direct(new SimpleBlockFeature(BlockStateProvider.of(Blocks.MELON)));
        var placed = new PlacedFeature(feature, List.of(CountPlacement.of(3), InSquarePlacement.spread()));
        var constructor = FeaturePoolElement.class.getDeclaredConstructor(Holder.class, StructureTemplatePool.Projection.class);
        constructor.setAccessible(true);
        var element = constructor.newInstance(Holder.direct(placed), StructureTemplatePool.Projection.RIGID);
        for (long seed : new long[]{0, 42, -1, 123456789}) {
            var writes = new ArrayList<BlockPos>();
            var world = (WorldGenLevel) Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(), new Class<?>[]{WorldGenLevel.class}, (proxy, method, arguments) -> {
                if (method.getName().equals("setBlock")) { writes.add((BlockPos) arguments[0]); return true; }
                if (method.getName().equals("getMinY")) return 0;
                if (method.getName().equals("getHeight")) return 16;
                throw new UnsupportedOperationException(method.toString());
            });
            var random = new WorldgenRandom(new XoroshiroRandomSource(seed));
            var origin = new BlockPos(1, 8, 1);
            var result = element.place(null, world, null, new Generator(), origin, BlockPos.ZERO, Rotation.CLOCKWISE_90,
                new BoundingBox(0, 0, 0, 15, 15, 15), random, LiquidSettings.IGNORE_WATERLOGGING, false);
            out.printf("CASE\t%d\t%s\t%d\t%d%n", seed, result, random.nextLong(), writes.size());
            for (var pos : writes) out.printf("WRITE\t%d\t%d\t%d%n", pos.getX(), pos.getY(), pos.getZ());
        }
    }
}
