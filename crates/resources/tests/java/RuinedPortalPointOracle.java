import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalPiece;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalStructure;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager;

public class RuinedPortalPointOracle {
    static class Generator extends RuinedPortalHeightOracle.Generator {
        Generator(BiomeSource source) { super(source); }
        public int getBaseHeight(int x, int z, Heightmap.Types map, LevelHeightAccessor a, RandomState r) {
            int floor = 60 + Math.floorMod(x * 13 + z * 7, 11) - 5;
            return floor + (map == Heightmap.Types.OCEAN_FLOOR_WG ? 1 : 4);
        }
    }
    public static void main(String[] args) throws Exception {
        var output = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        bind.setAccessible(true);
        bind.invoke(Blocks.STONE.builtInRegistryHolder(), List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP));
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var manager = (StructureTemplateManager) unsafe.allocateInstance(StructureTemplateManager.class);
        var repository = StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var sizeField = StructureTemplate.class.getDeclaredField("size"); sizeField.setAccessible(true);
        var biome = Holder.direct((Biome) unsafe.allocateInstance(Biome.class));
        var source = new FixedBiomeSource(biome);
        var generator = new Generator(source);
        var settings = new Structure.StructureSettings(HolderSet.direct(biome));
        long[] seeds = {0, 1, -1, 42, 123456789, Long.MIN_VALUE, Long.MAX_VALUE};
        for (var placement : RuinedPortalPiece.VerticalPlacement.values()) for (boolean pocket : new boolean[]{false,true})
            for (long seed : seeds) for (int i = 0; i < 11; i++) for (boolean allowed : new boolean[]{false,true}) {
                var template = new StructureTemplate();
                int sx = 5 + i, sy = 5 + i % 5, sz = 8 + (i * 3) % 11;
                sizeField.set(template, new Vec3i(sx, sy, sz));
                var templates = new HashMap<Identifier, Optional<StructureTemplate>>();
                for (int n = 1; n <= 10; n++) templates.put(Identifier.withDefaultNamespace("ruined_portal/portal_" + n), Optional.of(template));
                for (int n = 1; n <= 3; n++) templates.put(Identifier.withDefaultNamespace("ruined_portal/giant_portal_" + n), Optional.of(template));
                repository.set(manager, templates);
                int x = i * 19 - 80, z = 100 - i * 13;
                var random = new WorldgenRandom(new LegacyRandomSource(0)); random.setLargeFeatureSeed(seed, x, z);
                var context = new Structure.GenerationContext(null, generator, source, null,
                        (qx, qy, qz) -> biome, null, manager, random, seed, new ChunkPos(x, z),
                        LevelHeightAccessor.create(-64, 384), ignored -> allowed);
                var setup = new RuinedPortalStructure.Setup(placement, pocket ? 1.0f : 0.0f,
                        0.2f, false, false, false, false, 1.0f);
                var stub = new RuinedPortalStructure(settings, setup).findValidGenerationPoint(context);
                String prefix = "P " + placement.name() + " " + pocket + " " + seed + " " + x + " " + z + " " + sx + " " + sy + " " + sz + " " + allowed;
                if (stub.isEmpty()) { output.println(prefix + " X " + random.nextInt()); continue; }
                var point = stub.orElseThrow();
                var generatorClosure = point.generator().left().orElseThrow();
                Rotation rotation = null; Mirror mirror = null;
                for (var f : generatorClosure.getClass().getDeclaredFields()) {
                    f.setAccessible(true); var value = f.get(generatorClosure);
                    if (value instanceof Rotation r) rotation = r;
                    if (value instanceof Mirror m) mirror = m;
                }
                if (rotation == null || mirror == null) throw new AssertionError("missing transform in portal stub");
                var pos = point.position();
                var box = template.getBoundingBox(pos, rotation, new BlockPos(sx / 2, 0, sz / 2), mirror);
                output.printf("%s %d %d %d %d %d %d %d %d %d %d%n", prefix,
                        pos.getX(), pos.getY(), pos.getZ(), box.minX(), box.minY(), box.minZ(),
                        box.maxX(), box.maxY(), box.maxZ(), random.nextInt());
            }
    }
}
