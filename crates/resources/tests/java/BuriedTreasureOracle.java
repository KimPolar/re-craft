import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.structures.BuriedTreasurePieces.BuriedTreasurePiece;

public class BuriedTreasureOracle {
 static int[] p(JsonElement e){var a=e.getAsJsonArray();return new int[]{a.get(0).getAsInt(),a.get(1).getAsInt(),a.get(2).getAsInt()};}
 static BlockPos pos(JsonElement e){var v=p(e);return new BlockPos(v[0],v[1],v[2]);}
 static BlockState state(String name){return BuiltInRegistries.BLOCK.getValue(Identifier.parse(name)).defaultBlockState();}
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var fixtures=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
  for(var item:fixtures){var f=item.getAsJsonObject();int id=f.get("id").getAsInt();long seed=f.get("seed").getAsLong();int mode=f.get("mode").getAsInt();
   var blocks=new HashMap<BlockPos,BlockState>();for(var b:f.getAsJsonArray("blocks")){var v=b.getAsJsonObject();blocks.put(pos(v.get("position")),state(v.get("state").getAsString()));}
   var writes=new ArrayList<String>();var entities=new HashMap<BlockPos,ChestBlockEntity>();boolean createEntity=f.get("entity").getAsBoolean();
   WorldGenLevel world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(obj,method,a)->switch(method.getName()){
    case "getMinY" -> 0;case "getMaxY" -> 20;case "getHeight" -> 12;
    case "getBlockState" -> blocks.getOrDefault((BlockPos)a[0],Blocks.AIR.defaultBlockState());
    case "getBlockEntity" -> entities.get((BlockPos)a[0]);
    case "setBlockAndUpdate","setBlock" -> {var position=((BlockPos)a[0]).immutable();var block=(BlockState)a[1];blocks.put(position,block);String facing=block.is(Blocks.CHEST)?block.getValue(ChestBlock.FACING).getName():"-";
      writes.add("W "+position.getX()+" "+position.getY()+" "+position.getZ()+" "+BuiltInRegistries.BLOCK.getKey(block.getBlock())+" "+facing);
      if(block.is(Blocks.CHEST)&&createEntity)entities.put(position,new ChestBlockEntity(position,block));yield true;}
    default -> throw new UnsupportedOperationException(method.getName());
   });
   var lo=p(f.get("clip_min"));var hi=p(f.get("clip_max"));var clip=new BoundingBox(lo[0],lo[1],lo[2],hi[0],hi[1],hi[2]);
   var piece=new BuriedTreasurePiece(new BlockPos(9,90,9));RandomSource random=switch(mode){case 0->new LegacyRandomSource(seed);case 1->new XoroshiroRandomSource(seed);default->new WorldgenRandom(new XoroshiroRandomSource(seed));};
   piece.postProcess(world,null,null,random,clip,new ChunkPos(0,0),BlockPos.ZERO);var box=piece.getBoundingBox();
   out.println("S "+id);for(var line:writes)out.println(line);out.printf("B %d %d %d %d %d %d%n",box.minX(),box.minY(),box.minZ(),box.maxX(),box.maxY(),box.maxZ());
   for(var entry:entities.entrySet()){var chest=entry.getValue();if(chest.getLootTable()!=null)out.printf("C %d %d %d %s %d%n",entry.getKey().getX(),entry.getKey().getY(),entry.getKey().getZ(),chest.getLootTable().identifier(),chest.getLootTableSeed());}
   out.println("R "+random.nextLong());
  }
  for(long seed:new long[]{0,1,-1,Long.MIN_VALUE,Long.MAX_VALUE})for(int x:new int[]{0,-16,123456,Integer.MIN_VALUE,Integer.MAX_VALUE})for(int step:new int[]{0,1,17,Integer.MAX_VALUE}){
   int z=x^0x71374219,index=x^0x72634513;var random=new WorldgenRandom(new XoroshiroRandomSource(0));
   long decoration=random.setDecorationSeed(seed,x,z);random.setFeatureSeed(decoration,index,step);
   out.printf("D %d %d %d %d %d %d %d %d %d %s %d %d%n",seed,x,z,index,step,decoration,random.nextLong(),random.nextInt(),random.nextInt(1234567),random.nextBoolean(),Float.floatToRawIntBits(random.nextFloat()),Double.doubleToRawLongBits(random.nextDouble()));
  }
 }
}
