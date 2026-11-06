import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.charset.StandardCharsets;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.structure.pools.StructureTemplatePool;
import net.minecraft.world.level.levelgen.structure.pools.alias.*;

public class JigsawAliasOracle {
    static void aliases(JsonElement element, Set<String> names) {
        if (element.isJsonArray()) for (var child:element.getAsJsonArray()) aliases(child,names);
        if (element.isJsonObject()) {
            var obj=element.getAsJsonObject();
            if(obj.has("alias")) names.add(Identifier.parse(obj.get("alias").getAsString()).toString());
            for(var entry:obj.entrySet()) aliases(entry.getValue(),names);
        }
    }
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var stream=JigsawAliasOracle.class.getClassLoader().getResourceAsStream("data/minecraft/worldgen/structure/trial_chambers.json");
        var actual=JsonParser.parseString(new String(stream.readAllBytes(),StandardCharsets.UTF_8)).getAsJsonObject().get("pool_aliases");
        var synthetic=JsonParser.parseString("""
          [{"type":"minecraft:direct","alias":"example:fixed","target":"example:first"},
           {"type":"minecraft:random","alias":"example:heavy","targets":[{"data":"example:zero","weight":0},{"data":"example:a","weight":65},{"data":"example:b","weight":9}]},
           {"type":"minecraft:random_group","groups":[{"weight":2,"data":[{"type":"minecraft:direct","alias":"example:group","target":"example:c"},{"type":"minecraft:random","alias":"example:nested","targets":[{"weight":1,"data":"example:d"},{"weight":3,"data":"example:e"}]}]},{"weight":1,"data":[{"type":"minecraft:direct","alias":"example:group","target":"example:f"}]}]}]
          """);
        var cases=new JsonElement[]{actual,synthetic};
        for(int mode=0;mode<cases.length;mode++) {
            var json=cases[mode]; out.printf("D\t%d\t%s%n",mode,json.toString());
            var bindings=PoolAliasBinding.CODEC.listOf().parse(JsonOps.INSTANCE,json).getOrThrow();
            var names=new TreeSet<String>(); aliases(json,names);
            for(long seed:new long[]{0,1,-1,42,Long.MIN_VALUE,Long.MAX_VALUE}) for(int[] p:new int[][]{{0,0,0},{-1,-27,3},{200,80,-400},{Integer.MIN_VALUE,319,Integer.MAX_VALUE}}) {
                var pos=new BlockPos(p[0],p[1],p[2]);
                var lookup=PoolAliasLookup.create(bindings,pos,seed);
                var random=RandomSource.create(seed).forkPositional().at(pos);
                for(var binding:bindings) binding.forEachResolved(random,(a,b)->{});
                out.printf("S\t%d\t%d\t%d\t%d\t%d\t%d%n",mode,seed,p[0],p[1],p[2],random.nextLong());
                for(String name:names) {
                    ResourceKey<StructureTemplatePool> key=ResourceKey.create(Registries.TEMPLATE_POOL,Identifier.parse(name));
                    out.printf("P\t%d\t%d\t%d\t%d\t%d\t%s\t%s%n",mode,seed,p[0],p[1],p[2],name,lookup.lookup(key).identifier());
                }
            }
        }
    }
}
