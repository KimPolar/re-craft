import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.level.storage.loot.*;
import net.minecraft.world.level.storage.loot.predicates.LootItemRandomChanceCondition;

public class RandomChanceOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var ctor=LootContext.class.getDeclaredConstructor(LootParams.class,RandomSource.class,net.minecraft.core.HolderGetter.Provider.class); ctor.setAccessible(true);
    var rows=new JsonArray();
    for(float chance:new float[]{-0.5f,0.0f,0.125f,0.5f,1.0f,1.5f}) for(int seed=0;seed<128;seed++) {
      var random=RandomSource.create(seed);
      var context=ctor.newInstance(new LootParams(null,ContextMap.builder().build(),Map.of(),0.0f),random,null);
      var result=LootItemRandomChanceCondition.randomChance(chance).build().test(context);
      var row=new JsonArray(); row.add(result); row.add(random.nextInt()); rows.add(row);
    }
    Files.writeString(Path.of(args[0]),rows.toString());
  }
}
