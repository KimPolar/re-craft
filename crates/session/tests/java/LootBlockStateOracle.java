import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.advancements.predicates.BlockPredicate;
import net.minecraft.advancements.predicates.StatePropertiesPredicate;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.RandomSource;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SnowLayerBlock;
import net.minecraft.world.level.storage.loot.LootContext;
import net.minecraft.world.level.storage.loot.LootParams;
import net.minecraft.world.level.storage.loot.parameters.LootContextParams;
import net.minecraft.world.level.storage.loot.predicates.MatchBlock;

public class LootBlockStateOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var ctor=LootContext.class.getDeclaredConstructor(LootParams.class,RandomSource.class,net.minecraft.core.HolderGetter.Provider.class); ctor.setAccessible(true);
    var result=new JsonArray();
    for(int wanted=1;wanted<=8;wanted++) {
      var predicate=BlockPredicate.Builder.block().of(BuiltInRegistries.BLOCK,Blocks.SNOW)
        .setProperties(StatePropertiesPredicate.Builder.properties().hasProperty(SnowLayerBlock.LAYERS,wanted)).build();
      var condition=new MatchBlock(predicate);
      for(int actual=1;actual<=8;actual++) {
        var state=Blocks.SNOW.defaultBlockState().setValue(SnowLayerBlock.LAYERS,actual);
        var params=new LootParams(null,ContextMap.builder().set(LootContextParams.BLOCK_STATE,state).build(),Map.of(),0.0f);
        var context=ctor.newInstance(params,RandomSource.create(42L),null);
        result.add(condition.test(context));
      }
    }
    Files.writeString(Path.of(args[0]),result.toString());
  }
}
