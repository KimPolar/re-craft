import java.io.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.*;

public class GeneratedLootChestOracle {
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var lookup=VanillaRegistries.createWorldLookup();var registries=VanillaRegistries.createReloadableLookup(lookup);
  for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries))pending.apply();
  var dir=Path.of(args[1]);long[] seeds={0,1,-1,Long.MIN_VALUE,Long.MAX_VALUE};
  for(int i=0;i<seeds.length;i++){
   var pos=new BlockPos(-17+i,63-i,31+i);var id=Identifier.parse(i==4?"example:custom_table":"minecraft:chests/buried_treasure");
   var table=ResourceKey.create(Registries.LOOT_TABLE,id);var path=dir.resolve(args[0]+"-"+i+".nbt");
   if(args[0].equals("java")){
    var chest=new ChestBlockEntity(pos,Blocks.CHEST.defaultBlockState());chest.setLootTable(table,seeds[i]);
    try(var stream=new DataOutputStream(Files.newOutputStream(path))){NbtIo.write(chest.saveWithFullMetadata(registries),(DataOutput)stream);}
   }else{
    CompoundTag tag;try(var stream=new DataInputStream(Files.newInputStream(path))){tag=NbtIo.read((DataInput)stream);}
    var block=BlockEntity.loadStatic(pos,Blocks.CHEST.defaultBlockState(),tag,registries);
    if(!(block instanceof ChestBlockEntity chest)||!table.equals(chest.getLootTable())||chest.getLootTableSeed()!=seeds[i])throw new AssertionError("incorrect deferred chest "+i+" "+tag);
    var saved=chest.saveWithFullMetadata(registries);if(!tag.equals(saved))throw new AssertionError("roundtrip mismatch "+i+" "+tag+" "+saved);
   }
  }
  out.println("OK");
 }
}
