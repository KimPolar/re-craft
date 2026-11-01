import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.dimension.LevelStem;

public class BaseColumnOracle {
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
  for(var b:List.of(Blocks.STONE,Blocks.NETHERRACK,Blocks.END_STONE))bind.invoke(b.builtInRegistryHolder(),List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP));
  var registries=VanillaRegistries.createWorldLookup();var dims=WorldPresets.createNormalWorldDimensions(registries);
  var kinds=List.of(LevelStem.OVERWORLD,LevelStem.NETHER,LevelStem.END);
  int[][] positions={{0,0},{4,5},{5,4},{15,16},{-1,-1},{-17,31},{101,-77},{1040,17}};
  for(int d=0;d<3;d++)for(long seed:new long[]{0,12345}){
   var stem=dims.get(kinds.get(d)).orElseThrow();var generator=(NoiseBasedChunkGenerator)stem.generator();var settings=generator.generatorSettings().value();
   var random=RandomState.create(registries.lookupOrThrow(Registries.NOISE),seed,settings);
   var dimension=stem.type().value();var accessor=LevelHeightAccessor.create(dimension.minY(),dimension.height());var noise=settings.noiseSettings().clampToHeightAccessor(accessor);
   for(var p:positions){int x=p[0],z=p[1];var column=generator.getBaseColumn(x,z,accessor,random);var states=new StringBuilder();
    for(int y=noise.minY();y<noise.minY()+noise.height();y++){
     var state=column.getBlock(y);char c=state.isAir()?'A':state.is(Blocks.WATER)?'W':state.is(Blocks.LAVA)?'L':state.is(Blocks.STONE)?'S':state.is(Blocks.NETHERRACK)?'N':state.is(Blocks.END_STONE)?'E':'?';
     if(c=='?')throw new AssertionError("unexpected "+state);states.append(c);
    }
    int surface=generator.getFirstOccupiedHeight(x,z,Heightmap.Types.WORLD_SURFACE_WG,accessor,random);
    int floor=generator.getFirstOccupiedHeight(x,z,Heightmap.Types.OCEAN_FLOOR_WG,accessor,random);
    out.printf("P %d %d %d %d %d %d %d %d %s%n",d,seed,x,z,noise.minY(),noise.height(),surface,floor,states);
   }
  }
 }
}
