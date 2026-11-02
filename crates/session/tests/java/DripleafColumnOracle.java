import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.core.Direction;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.EnumProperty;
import net.minecraft.util.valueproviders.ConstantInt;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.BlockColumnFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import java.lang.reflect.Proxy;
import java.util.function.Predicate;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.properties.BooleanProperty;
import net.minecraft.world.level.block.state.properties.Property;
import java.util.*;

public class DripleafColumnOracle {
  static String key(net.minecraft.world.level.block.state.BlockState state) {
    var parts=new ArrayList<String>();
    state.getValues().forEach(value->parts.add(value.property().getName()+"="+value.valueName()));
    Collections.sort(parts); return String.join(",",parts);
  }
  public static void main(String[] args){
    var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    for(var block:List.of(Blocks.BIG_DRIPLEAF,Blocks.BIG_DRIPLEAF_STEM,Blocks.SMALL_DRIPLEAF)){
      var name=BuiltInRegistries.BLOCK.getKey(block);
      for(var state:block.getStateDefinition().getPossibleStates()){
        var post=state.getPostProcessPos(EmptyBlockGetter.INSTANCE,BlockPos.ZERO);
        out.printf("S %s %s %s %s %s %s %s %s %s %s %s%n",name,key(state),state.isSolid(),state.hasBlockEntity(),PoiTypes.hasPoi(state),post==null ? "none" : post.toShortString(),state.getFluidState().isEmpty(),
          Heightmap.Types.WORLD_SURFACE.isOpaque().test(state),Heightmap.Types.OCEAN_FLOOR.isOpaque().test(state),
          Heightmap.Types.MOTION_BLOCKING.isOpaque().test(state),Heightmap.Types.MOTION_BLOCKING_NO_LEAVES.isOpaque().test(state));
      }
    }
    for(boolean wet:new boolean[]{false,true}) for(Direction facing:Direction.Plane.HORIZONTAL){
      var stem=Blocks.BIG_DRIPLEAF_STEM.defaultBlockState();
      var head=Blocks.BIG_DRIPLEAF.defaultBlockState();
      var sp=stem.getBlock().getStateDefinition();
      var hp=head.getBlock().getStateDefinition();
      stem=stem.setValue((EnumProperty<Direction>)sp.getProperty("facing"),facing).setValue((BooleanProperty)sp.getProperty("waterlogged"),wet);
      head=head.setValue((EnumProperty<Direction>)hp.getProperty("facing"),facing).setValue((BooleanProperty)hp.getProperty("waterlogged"),wet);
      var feature=new BlockColumnFeature(List.of(
        BlockColumnFeature.layer(ConstantInt.of(2),BlockStateProvider.of(stem)),
        BlockColumnFeature.layer(ConstantInt.of(1),BlockStateProvider.of(head))),
        Direction.UP,BlockPredicate.matchesBlocks(Blocks.AIR),true);
      var changes=new LinkedHashMap<BlockPos,BlockState>();
      var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
        var pos=(BlockPos)a[0];var current=changes.getOrDefault(pos,Blocks.AIR.defaultBlockState());
        return switch(m.getName()){
          case "getBlockState"->current;
          case "isStateAtPosition"->((Predicate<BlockState>)a[1]).test(current);
          case "setBlock"->{changes.put(pos.immutable(),(BlockState)a[1]);yield true;}
          default->throw new UnsupportedOperationException(m.toString());
        };
      });
      var random=new WorldgenRandom(new XoroshiroRandomSource(42));
      boolean result=feature.place(world,null,random,new BlockPos(8,8,8));
      out.printf("C %s %s %s %d%n",facing.getSerializedName(),wet,result,random.nextLong());
      for(var entry:changes.entrySet()) out.printf("B %d %d %d %s %s%n",entry.getKey().getX(),entry.getKey().getY(),entry.getKey().getZ(),BuiltInRegistries.BLOCK.getKey(entry.getValue().getBlock()),key(entry.getValue()));
    }
  }
}
