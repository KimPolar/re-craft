import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.dimension.LevelStem;

public class TerrainChunkOracle {
  static java.io.PrintStream output;
  static final IdentityHashMap<BlockState,Integer> palette=new IdentityHashMap<>();
  static String key(BlockState state) {
    var properties=new TreeMap<String,String>();
    state.getValues().forEach(entry->properties.put(entry.property().getName(),entry.valueName()));
    var result=new StringBuilder(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()).append('|');
    for(var e:properties.entrySet())result.append(e.getKey()).append('=').append(e.getValue()).append(';');
    return result.toString();
  }
  static int stateIndex(BlockState state) {
    Integer index=palette.get(state);if(index!=null)return index;
    int next=palette.size();palette.put(state,next);output.println("L "+next+" "+key(state));return next;
  }
  static Method method(String name) {
    var m=Arrays.stream(NoiseBasedChunkGenerator.class.getDeclaredMethods()).filter(x->x.getName().equals(name)).findFirst().orElseThrow();
    m.setAccessible(true);return m;
  }
  static void dump(String stage,ProtoChunk chunk,int min,int height) {
    output.println("S "+stage);
    var pos=new BlockPos.MutableBlockPos();
    for(int z=0;z<16;z++)for(int x=0;x<16;x++) {
      var row=new StringBuilder();
      for(int y=min;y<min+height;y++) {
        var state=chunk.getBlockState(pos.set(chunk.getPos().getBlockX(x),y,chunk.getPos().getBlockZ(z)));
        if(y>min)row.append(',');row.append(stateIndex(state));
      }
      output.println("P "+x+" "+z+" "+row);
    }
    var scheduled=new BitSet(256*height);var lists=chunk.getPostProcessing();
    for(int section=0;section<lists.length;section++)if(lists[section]!=null)for(short packed:lists[section]) {
      var p=ProtoChunk.unpackOffsetCoordinates(packed,chunk.getSectionYFromSectionIndex(section),chunk.getPos());
      int y=p.getY()-min;if(y>=0 && y<height)scheduled.set(((p.getZ()&15)*16+(p.getX()&15))*height+y);
    }
    var words=scheduled.toLongArray();var line=new StringBuilder("F");for(long word:words)line.append(' ').append(Long.toUnsignedString(word,16));output.println(line);
  }
  public static void main(String[] args)throws Exception {
    var out=System.out;output=out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);bind.invoke(Blocks.BEDROCK.builtInRegistryHolder(),List.of(BlockTags.UNCARVABLE));
    var registries=VanillaRegistries.createWorldLookup();
    var dimensions=WorldPresets.createNormalWorldDimensions(registries);
    for(int arg=0;arg<args.length;arg+=4){
    var dimension=switch(args[arg]) {case "overworld"->LevelStem.OVERWORLD;case "nether"->LevelStem.NETHER;case "end"->LevelStem.END;default->throw new IllegalArgumentException(args[arg]);};
    var stem=dimensions.get(dimension).orElseThrow();
    var generator=(NoiseBasedChunkGenerator)stem.generator();var settings=generator.generatorSettings().value();
    long seed=Long.parseLong(args[arg+1]);int cx=Integer.parseInt(args[arg+2]),cz=Integer.parseInt(args[arg+3]);
    var random=RandomState.create(registries.lookupOrThrow(Registries.NOISE),seed,settings);
    var resolver=generator.getBiomeSource().createUncachedResolver(random);
    var biomeManager=new BiomeManager(resolver,BiomeManager.obfuscateSeed(seed));
    var plains=registries.lookupOrThrow(Registries.BIOME).getOrThrow(Biomes.PLAINS);
    var ids=new IdMapper<Holder<Biome>>();ids.add(plains);
    var palette=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),plains,null);
    var dim=stem.type().value();var height=LevelHeightAccessor.create(dim.minY(),dim.height());
    var chunk=new ProtoChunk(new ChunkPos(cx,cz),UpgradeData.EMPTY,height,palette,null);
    var ns=settings.noiseSettings().clampToHeightAccessor(height);
    var picker=(Aquifer.FluidPicker)(x,y,z)->y<Math.min(-54,settings.seaLevel())?new Aquifer.FluidStatus(-54,Blocks.LAVA.defaultBlockState()):new Aquifer.FluidStatus(settings.seaLevel(),settings.defaultFluid());
    var volume=new DensityVolume(16,ns.height(),16,cx*16,ns.minY(),cz*16);
    try(var noise=new NoiseChunk(random,null,settings,picker,Blender.empty(),volume)) {
      method("doFill").invoke(generator,noise,chunk);
      method("buildSurface").invoke(generator,chunk,noise,random,biomeManager,generator.getBiomeSource().possibleBiomes(),settings.materialRule().value());
      dump("surface",chunk,dim.minY(),dim.height());
      method("generateCarvers").invoke(generator,chunk,Blender.empty(),noise,random,biomeManager,null,settings.materialRule().value());
      dump("carvers",chunk,dim.minY(),dim.height());
    }
    }
    for(var block:BuiltInRegistries.BLOCK)out.println("R "+key(block.defaultBlockState()));
  }
}
