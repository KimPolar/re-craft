import com.mojang.datafixers.util.Pair;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.structure.pools.StructurePoolElement;
import net.minecraft.world.level.levelgen.structure.pools.StructureTemplatePool;

public class JigsawPoolOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var projection = StructureTemplatePool.Projection.RIGID;
        var a = StructurePoolElement.single("example:a").apply(projection);
        var b = StructurePoolElement.single("example:b").apply(projection);
        var c = StructurePoolElement.empty().apply(projection);
        var pool = new StructureTemplatePool(null, List.of(Pair.of(a, 2), Pair.of(b, 3), Pair.of(c, 1)));
        for (int mode = 0; mode < 2; mode++) {
            for (long seed : new long[] {0, 1, -1, 42, Long.MIN_VALUE, Long.MAX_VALUE}) {
                RandomSource random = mode == 0
                    ? new LegacyRandomSource(seed)
                    : new WorldgenRandom(new XoroshiroRandomSource(seed));
                var chosen = pool.getRandomTemplate(random);
                var shuffled = pool.getShuffledTemplates(random);
                var order = new StringBuilder();
                for (var element : shuffled) order.append(element == a ? 'a' : element == b ? 'b' : 'c');
                out.println("P\t" + mode + "\t" + seed + "\t" + (chosen == a ? 'a' : chosen == b ? 'b' : 'c') + "\t" + order + "\t" + random.nextLong());
            }
        }
    }
}
