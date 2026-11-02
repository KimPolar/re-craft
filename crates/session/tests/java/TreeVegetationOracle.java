import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.LevelAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.block.state.properties.DoubleBlockHalf;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;

public class TreeVegetationOracle {
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
    for(String name:List.of("short_grass","fern","bush","dandelion","poppy","blue_orchid","allium","azure_bluet","red_tulip","orange_tulip","white_tulip","pink_tulip","oxeye_daisy","cornflower","lily_of_the_valley","firefly_bush","closed_eyeblossom","sweet_berry_bush","short_dry_grass","tall_dry_grass","dead_bush","tall_grass","large_fern","sunflower","rose_bush","peony","lilac","oak_sapling","spruce_sapling","birch_sapling","jungle_sapling","acacia_sapling","dark_oak_sapling","cherry_sapling","pale_oak_sapling")){
      var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+name));
      boolean doubled=block.defaultBlockState().hasProperty(BlockStateProperties.DOUBLE_BLOCK_HALF);
      int stages=name.endsWith("_sapling")?2:name.equals("sweet_berry_bush")?4:1;
      for(boolean support:new boolean[]{false,true})for(int stage=0;stage<stages;stage++)for(int half=0;half<(doubled?2:1);half++)for(int belowPair=0;belowPair<(doubled&&half==1?2:1);belowPair++)for(Direction direction:List.of(Direction.DOWN,Direction.UP,Direction.EAST))for(int neighbor=0;neighbor<(doubled?3:1);neighbor++){
        bind.invoke(Blocks.STONE.builtInRegistryHolder(),support?List.of(BlockTags.SUPPORTS_VEGETATION,BlockTags.SUPPORTS_DRY_VEGETATION):List.of());
        var state=block.defaultBlockState();if(doubled)state=state.setValue(BlockStateProperties.DOUBLE_BLOCK_HALF,half==0?DoubleBlockHalf.LOWER:DoubleBlockHalf.UPPER);
        if(name.endsWith("_sapling"))state=state.setValue(BlockStateProperties.STAGE,stage);
        if(name.equals("sweet_berry_bush"))state=state.setValue(BlockStateProperties.AGE_3,stage);
        BlockState under=belowPair==1?block.defaultBlockState().setValue(BlockStateProperties.DOUBLE_BLOCK_HALF,DoubleBlockHalf.LOWER):Blocks.STONE.defaultBlockState();
        BlockState adjacent=neighbor==0?Blocks.AIR.defaultBlockState():block.defaultBlockState().setValue(BlockStateProperties.DOUBLE_BLOCK_HALF,neighbor==1?(half==0?DoubleBlockHalf.UPPER:DoubleBlockHalf.LOWER):(half==0?DoubleBlockHalf.LOWER:DoubleBlockHalf.UPPER));
        var world=(LevelAccessor)Proxy.newProxyInstance(LevelAccessor.class.getClassLoader(),new Class[]{LevelAccessor.class},(proxy,m,v)->{
          if(m.getName().equals("getBlockState"))return under;
          if(m.getName().equals("scheduleTick"))throw new AssertionError("unexpected vegetation tick");
          throw new UnsupportedOperationException(m.toString());
        });
        var p=new BlockPos(8,8,8);
        var updated=state.updateShape(world,world,p,direction,p.relative(direction),adjacent,new WorldgenRandom(new XoroshiroRandomSource(0)));
        var extra=updated.hasProperty(BlockStateProperties.DOUBLE_BLOCK_HALF)?","+updated.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF):updated.hasProperty(BlockStateProperties.STAGE)?",stage="+updated.getValue(BlockStateProperties.STAGE):updated.hasProperty(BlockStateProperties.AGE_3)?",age="+updated.getValue(BlockStateProperties.AGE_3):"";
        out.printf("C %s %s %d %d %d %s %d %s%s%n",name,support,stage,half,belowPair,direction,neighbor,BuiltInRegistries.BLOCK.getKey(updated.getBlock()),extra);
      }
    }
  }
}
