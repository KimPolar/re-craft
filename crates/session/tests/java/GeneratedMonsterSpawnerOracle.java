import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.*;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;

public class GeneratedMonsterSpawnerOracle {
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var lookup=VanillaRegistries.createWorldLookup();var registries=VanillaRegistries.createReloadableLookup(lookup);
  for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries))pending.apply();
  var mobs=List.of(EntityTypes.SKELETON,EntityTypes.ZOMBIE,EntityTypes.SPIDER,EntityTypes.CAVE_SPIDER);
  for(int i=0;i<mobs.size();i++){
   var pos=new BlockPos(-17+i,63-i,31+i);var path=Path.of(args[1]).resolve(args[0]+"-"+i+".nbt");
   if(args[0].equals("java")){
    var spawner=new SpawnerBlockEntity(pos,Blocks.SPAWNER.defaultBlockState());
    var random=new WorldgenRandom(new XoroshiroRandomSource(1));spawner.setEntityId(mobs.get(i),random);
    out.printf("R %d %d%n",i,random.nextLong());
    try(var stream=new DataOutputStream(Files.newOutputStream(path))){NbtIo.write(spawner.saveWithFullMetadata(registries),(DataOutput)stream);}
   }else{
    CompoundTag tag;try(var stream=new DataInputStream(Files.newInputStream(path))){tag=NbtIo.read((DataInput)stream);}
    var block=BlockEntity.loadStatic(pos,Blocks.SPAWNER.defaultBlockState(),tag,registries);
    if(!(block instanceof SpawnerBlockEntity spawner))throw new AssertionError("incorrect spawner "+tag);
    var saved=spawner.saveWithFullMetadata(registries);if(!tag.equals(saved))throw new AssertionError("roundtrip mismatch "+i+" "+tag+" "+saved);
   }
  }
  out.println("OK");
 }
}
