import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.Field;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.heightproviders.HeightProvider;

public class HeightProviderOracle {
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        out.printf("B\t%d\t%d\t%d%n",DimensionType.MIN_Y,DimensionType.MAX_Y,DimensionType.Y_SIZE);
        Field uf=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); uf.setAccessible(true); var unsafe=(sun.misc.Unsafe)uf.get(null);
        var fields=new Field[]{WorldGenerationContext.class.getDeclaredField("minY"),WorldGenerationContext.class.getDeclaredField("height"),WorldGenerationContext.class.getDeclaredField("seaLevel")};
        for(var f:fields) f.setAccessible(true);
        String[] inputs={
          "{\"absolute\":-27}",
          "{\"above_bottom\":12}",
          "{\"below_top\":8}",
          "{\"relative_to_sea_level\":-10}",
          "{\"type\":\"minecraft:constant\",\"value\":{\"absolute\":14}}",
          "{\"type\":\"minecraft:uniform\",\"min_inclusive\":{\"absolute\":-40},\"max_inclusive\":{\"absolute\":-20}}",
          "{\"type\":\"minecraft:uniform\",\"min_inclusive\":{\"absolute\":3},\"max_inclusive\":{\"absolute\":3}}",
          "{\"type\":\"minecraft:uniform\",\"min_inclusive\":{\"absolute\":9},\"max_inclusive\":{\"absolute\":1}}",
          "{\"type\":\"minecraft:trapezoid\",\"min_inclusive\":{\"above_bottom\":2},\"max_inclusive\":{\"relative_to_sea_level\":0},\"plateau\":4}",
          "{\"type\":\"minecraft:trapezoid\",\"min_inclusive\":{\"absolute\":3},\"max_inclusive\":{\"absolute\":8},\"plateau\":-2}",
          "{\"type\":\"minecraft:biased_to_bottom\",\"min_inclusive\":{\"absolute\":-20},\"max_inclusive\":{\"absolute\":20},\"inner\":3}",
          "{\"type\":\"minecraft:very_biased_to_bottom\",\"min_inclusive\":{\"above_bottom\":3},\"max_inclusive\":{\"below_top\":30}}",
          "{\"type\":\"minecraft:very_biased_to_bottom\",\"min_inclusive\":{\"absolute\":3},\"max_inclusive\":{\"absolute\":3},\"inner\":2}",
          "{\"type\":\"minecraft:weighted_list\",\"distribution\":[{\"data\":{\"absolute\":1},\"weight\":0},{\"data\":{\"absolute\":4},\"weight\":70},{\"data\":{\"type\":\"minecraft:uniform\",\"min_inclusive\":{\"absolute\":0},\"max_inclusive\":{\"absolute\":9}},\"weight\":3}]}",
          "{\"absolute\":7,\"type\":\"minecraft:uniform\"}"
        };
        for(int id=0;id<inputs.length;id++) {
            out.printf("D\t%d\t%s%n",id,inputs[id]);
            var provider=HeightProvider.CODEC.parse(JsonOps.INSTANCE,JsonParser.parseString(inputs[id])).getOrThrow();
            for(int[] dimensions:new int[][]{{-64,384,63},{0,128,32},{-100,80,-5}}) for(long seed:new long[]{0,1,-1,42,Long.MIN_VALUE,Long.MAX_VALUE}) {
                var context=(WorldGenerationContext)unsafe.allocateInstance(WorldGenerationContext.class);
                for(int i=0;i<3;i++) fields[i].setInt(context,dimensions[i]);
                var random=new LegacyRandomSource(seed);
                int result=provider.sample(random,context);
                out.printf("S\t%d\t%d\t%d\t%d\t%d\t%d\t%d%n",id,dimensions[0],dimensions[1],dimensions[2],seed,result,random.nextLong());
            }
        }
    }
}
