import java.util.*;
import java.util.function.Function;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalPiece;
import net.minecraft.world.level.levelgen.structure.structures.RuinedPortalStructure;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager;

public class RuinedPortalSelectionOracle {
    static class Selected extends RuntimeException {
        final Identifier template;
        Selected(Identifier template) { this.template = template; }
    }
    static class Capture extends HashMap<Identifier, Optional<StructureTemplate>> {
        @Override public Optional<StructureTemplate> computeIfAbsent(
                Identifier key, Function<? super Identifier, ? extends Optional<StructureTemplate>> fn) {
            throw new Selected(key);
        }
    }
    static RuinedPortalStructure.Setup setup(float probability, float weight) {
        return new RuinedPortalStructure.Setup(RuinedPortalPiece.VerticalPlacement.ON_LAND_SURFACE,
                probability, 0.2f, false, false, false, false, weight);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) field.get(null);
        var manager = (StructureTemplateManager) unsafe.allocateInstance(StructureTemplateManager.class);
        var repository = StructureTemplateManager.class.getDeclaredField("structureRepository");
        repository.setAccessible(true); repository.set(manager, new Capture());
        var biome = Holder.direct((Biome) unsafe.allocateInstance(Biome.class));
        var settings = new Structure.StructureSettings(HolderSet.direct(biome));
        long[] seeds = {0, 1, -1, 42, 123456789, Long.MIN_VALUE, Long.MAX_VALUE};
        for (int mode = 0; mode < 5; mode++) for (long seed : seeds) for (int i = 0; i < 33; i++) {
            var setups = switch (mode) {
                case 0 -> List.of(setup(0.0f, 1.0f));
                case 1 -> List.of(setup(1.0f, 1.0f));
                case 2 -> List.of(setup(0.35f, 1.0f));
                case 3 -> List.of(setup(0.0f, 0.5f), setup(0.35f, 0.5f));
                default -> List.of(setup(1.0f, 0.1f), setup(0.0f, 0.3f), setup(0.35f, 0.6f));
            };
            int x = i * 19 - 300, z = 210 - i * 17;
            var random = new WorldgenRandom(new LegacyRandomSource(0));
            random.setLargeFeatureSeed(seed, x, z);
            var context = new Structure.GenerationContext(null, null, null, null, null, null,
                    manager, random, seed, new ChunkPos(x, z), LevelHeightAccessor.create(-64, 384), ignored -> true);
            Identifier chosen;
            try {
                new RuinedPortalStructure(settings, setups).findGenerationPoint(context);
                throw new AssertionError("template lookup was not reached");
            } catch (Selected selection) {
                chosen = selection.template;
            }
            int rotation = Rotation.getRandom(random).ordinal();
            int mirror = random.nextFloat() < 0.5f ? 0 : 1;
            System.out.printf("C %d %d %d %d %s %d %d %d%n",
                    mode, seed, x, z, chosen, rotation, mirror, random.nextInt());
        }
    }
}
