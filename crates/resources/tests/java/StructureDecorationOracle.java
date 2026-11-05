import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.structure.Structure;

public class StructureDecorationOracle {
    static Identifier fileKey(Identifier id) {
        return Identifier.fromNamespaceAndPath(id.getNamespace(), "worldgen/structure/" + id.getPath() + ".json");
    }
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var registry = VanillaRegistries.createWorldLookup().lookupOrThrow(Registries.STRUCTURE);
        List<Holder.Reference<Structure>> values = registry.listElements()
            .sorted(Comparator.comparing(h -> fileKey(h.key().identifier()))).toList();
        var steps = new ArrayList<List<Holder.Reference<Structure>>>();
        for (int i = 0; i < 11; i++) steps.add(new ArrayList<>());
        for (var holder : values) steps.get(holder.value().step().ordinal()).add(holder);
        long[] seeds = {0L, -1L, 123456789L, Long.MIN_VALUE, Long.MAX_VALUE};
        for (int step = 0; step < steps.size(); step++) {
            for (int index = 0; index < steps.get(step).size(); index++) {
                var entry = steps.get(step).get(index);
                for (long seed : seeds) {
                    var random = new WorldgenRandom(new XoroshiroRandomSource(0));
                    random.setFeatureSeed(seed, index, step);
                    out.printf("S %d %d %s %d %d %d %d%n", step, index,
                        entry.key().identifier(), seed, random.nextLong(), random.nextLong(), random.nextLong());
                }
            }
        }
    }
}
