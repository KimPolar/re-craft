import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.storage.TagValueInput;

public class JigsawEmptyChestOracle {
  public static void main(String[] args) throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var lookup=VanillaRegistries.createWorldLookup();
    var registries=VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
    CompoundTag tag=null;
    try(var stream=JigsawEmptyChestOracle.class.getClassLoader().getResourceAsStream("data/minecraft/structure/shipwreck/with_mast.nbt")) {
      var root=NbtIo.readCompressed(stream,NbtAccounter.unlimitedHeap());var blocks=root.getListOrEmpty("blocks");
      for(int i=0;i<blocks.size();i++){var nbt=blocks.getCompoundOrEmpty(i).getCompound("nbt");if(nbt.isPresent()&&nbt.get().getStringOr("id","").equals("minecraft:chest")){tag=nbt.get().copy();break;}}
    }
    if(tag==null) throw new AssertionError("missing shipwreck chest");
    var chest=new ChestBlockEntity(new BlockPos(1,8,1),Blocks.CHEST.defaultBlockState());
    var random=new WorldgenRandom(new XoroshiroRandomSource(42L));long first=random.nextLong();
    tag.putLong("LootTableSeed",first);
    chest.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING,registries,tag));
    var saved=chest.saveWithFullMetadata(registries);
    if(chest.getLootTable()!=null || saved.contains("LootTable") || saved.contains("LootTableSeed")) throw new AssertionError(saved);
    NbtIo.write(saved,Path.of(args[0]));
    out.printf("E %d %d%n",first,random.nextLong());
  }
}
