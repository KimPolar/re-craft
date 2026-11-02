import java.lang.reflect.*;
import java.util.*;
import java.util.function.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.BaseSpawner;
import net.minecraft.world.level.SpawnData;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.MonsterRoomFeature;

public class MonsterRoomOracle {
 static String pos(BlockPos p){return p.getX()+","+p.getY()+","+p.getZ();}
 static String name(BlockState s){if(s.is(Blocks.CAVE_AIR))return "air";if(s.is(Blocks.COBBLESTONE))return "cobble";if(s.is(Blocks.MOSSY_COBBLESTONE))return "mossy";if(s.is(Blocks.CHEST))return "chest:"+s.getValue(BlockStateProperties.HORIZONTAL_FACING);if(s.is(Blocks.SPAWNER))return "spawner";throw new AssertionError(s);}
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);bind.invoke(Blocks.BEDROCK.builtInRegistryHolder(),List.of(BlockTags.FEATURES_CANNOT_REPLACE));
  out.printf("K %s %s %s %s%n",Blocks.CHEST.defaultBlockState().isSolid(),Blocks.CHEST.defaultBlockState().isSolidRender(),Blocks.SPAWNER.defaultBlockState().isSolid(),Blocks.SPAWNER.defaultBlockState().isSolidRender());
  var next=BaseSpawner.class.getDeclaredField("nextSpawnData");next.setAccessible(true);
  boolean grass=args.length>0 && args[0].equals("chunk_grass");
  boolean chunk=grass || args.length>0 && args[0].equals("chunk");final int bx=chunk?8:0,by=chunk?8:20,bz=chunk?8:0;
  for(long seed:new long[]{0,1,-1,987654321})for(int mode=0;mode<8;mode++){
   final int scenario=mode;var probe=new WorldgenRandom(new XoroshiroRandomSource(seed));int rx=probe.nextInt(2)+2,rz=probe.nextInt(2)+2;
   var blocks=new HashMap<BlockPos,BlockState>();var entities=new HashMap<BlockPos,BlockEntity>();var writes=new ArrayList<String>();
   BiFunction<BlockPos,Integer,BlockState> initial=(p,unused)->{
    int x=p.getX()-bx,y=p.getY()-by,z=p.getZ()-bz;int kind=scenario%4;if(x==-rx-1 && z==0 && (y==0||y==1))return Blocks.AIR.defaultBlockState();
    if(kind==1 && x==-rx-1 && y==-1 && z==-rz-1)return Blocks.AIR.defaultBlockState();
    if(kind==2 && Math.abs(x)==rx+1 && Math.abs(z)<=rz && (y==0||y==1))return Blocks.AIR.defaultBlockState();
    if(kind==3 && ((x==-rx-1 && y==-1 && z==1)||(x==1&&y==0&&z==1)))return Blocks.BEDROCK.defaultBlockState();
    return grass?Blocks.GRASS_BLOCK.defaultBlockState():Blocks.STONE.defaultBlockState();
   };
   var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(proxy,m,v)->{
    if(m.getName().equals("getMinY"))return 0;
    if(m.getName().equals("getBlockState")||m.getName().equals("isEmptyBlock")){var p=(BlockPos)v[0];var s=blocks.getOrDefault(p,initial.apply(p,0));return m.getName().equals("isEmptyBlock")?s.isAir():s;}
    if(m.getName().equals("setBlock")){var p=((BlockPos)v[0]).immutable();var s=(BlockState)v[1];if((int)v[2]!=2)throw new AssertionError("flags");blocks.put(p,s);writes.add(pos(p)+","+name(s));entities.remove(p);if(scenario<4){if(s.is(Blocks.CHEST))entities.put(p,new ChestBlockEntity(p,s));if(s.is(Blocks.SPAWNER))entities.put(p,new SpawnerBlockEntity(p,s));}return true;}
    if(m.getName().equals("getBlockEntity"))return entities.get((BlockPos)v[0]);
    throw new UnsupportedOperationException(m.toString());
   });
   var random=new WorldgenRandom(new XoroshiroRandomSource(seed));boolean ok=new MonsterRoomFeature().place(world,null,random,new BlockPos(bx,by,bz));
   var chest=new ArrayList<String>();String mob="-";
   for(var entry:entities.entrySet())if(entry.getValue() instanceof ChestBlockEntity c && c.getLootTable()!=null)chest.add(pos(entry.getKey())+","+c.getLootTableSeed());else if(entry.getValue() instanceof SpawnerBlockEntity s){var data=(SpawnData)next.get(s.getSpawner());if(data!=null)mob=data.entityToSpawn().getString("id").orElse("?");}
   Collections.sort(chest);out.printf("C %d %d %s W %s T %s M %s R %d%n",seed,mode,ok,String.join(";",writes),String.join(";",chest),mob,random.nextLong());
  }
 }
}
