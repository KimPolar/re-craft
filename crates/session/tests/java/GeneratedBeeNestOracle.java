import java.io.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.*;

public class GeneratedBeeNestOracle {
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var lookup=VanillaRegistries.createWorldLookup();var registries=VanillaRegistries.createReloadableLookup(lookup);
  for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries))pending.apply();
  int[][] ticks={{0,598},{1,2,3},{598,0,300},{400,599-1}};
  for(int i=0;i<ticks.length;i++){
   var pos=new BlockPos(-17+i,63-i,31+i);var path=Path.of(args[1]).resolve(args[0]+"-"+i+".nbt");
   if(args[0].equals("java")){
    var nest=new BeehiveBlockEntity(pos,Blocks.BEE_NEST.defaultBlockState());
    for(int tick:ticks[i])nest.storeBee(BeehiveBlockEntity.Occupant.create(tick));
    try(var stream=new DataOutputStream(Files.newOutputStream(path))){NbtIo.write(nest.saveWithFullMetadata(registries),(DataOutput)stream);}
   }else{
    CompoundTag tag;try(var stream=new DataInputStream(Files.newInputStream(path))){tag=NbtIo.read((DataInput)stream);}
    var block=BlockEntity.loadStatic(pos,Blocks.BEE_NEST.defaultBlockState(),tag,registries);
    if(!(block instanceof BeehiveBlockEntity nest))throw new AssertionError("incorrect nest "+tag);
    var saved=nest.saveWithFullMetadata(registries);if(!tag.equals(saved))throw new AssertionError("roundtrip mismatch "+i+" "+tag+" "+saved);
   }
  }
  out.println("OK");
 }
}
