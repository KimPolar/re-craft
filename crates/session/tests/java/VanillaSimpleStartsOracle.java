import com.google.gson.JsonParser;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.*;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.tags.*;
import net.minecraft.resources.*;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.validation.DirectoryValidator;

public class VanillaSimpleStartsOracle {
 public static void main(String[] args)throws Exception{
  var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
  var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class);bind.setAccessible(true);
  for(var b:List.of(net.minecraft.world.level.block.Blocks.STONE,net.minecraft.world.level.block.Blocks.NETHERRACK,net.minecraft.world.level.block.Blocks.END_STONE))bind.invoke(b.builtInRegistryHolder(),List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP));
  var registries=VanillaRegistries.createWorldLookup();var biomeLookup=registries.lookupOrThrow(Registries.BIOME);
  var repo=ServerPacksSource.createPackRepository(Path.of(args[0]),new DirectoryValidator(path->false));
  MinecraftServer.configurePackRepository(repo,new WorldDataConfiguration(new DataPackConfig(List.of("vanilla"),List.of()),FeatureFlags.DEFAULT_FLAGS),false,false);
  try(var manager=new MultiPackResourceManager(PackType.SERVER_DATA,repo.openAllSelected())){
   TagLoader.ElementLookup<Holder<Biome>> lookup=(id,required)->biomeLookup.get(ResourceKey.create(Registries.BIOME,id)).map(h->(Holder<Biome>)h);
   var tags=TagLoader.loadTagsForRegistry(manager,Registries.BIOME,lookup);
   var generator=(NoiseBasedChunkGenerator)WorldPresets.getNormalOverworld(registries).generator();
   long seed=12345;var random=RandomState.create(registries.lookupOrThrow(Registries.NOISE),seed,generator.generatorSettings().value());
   var climate=random.createClimateSampler(SamplerContext.builder().enableCaches().build());var accessor=LevelHeightAccessor.create(-64,384);
   var cases=JsonParser.parseString(Files.readString(Path.of(args[1]))).getAsJsonArray();
   for(var item:cases){var obj=item.getAsJsonObject();String id=obj.get("structure").getAsString();int x=obj.get("x").getAsInt(),z=obj.get("z").getAsInt();
    var structure=registries.lookupOrThrow(Registries.STRUCTURE).getOrThrow(ResourceKey.create(Registries.STRUCTURE,Identifier.parse(id))).value();
    var allowed=structure.biomes().unwrapKey().map(key->Set.copyOf(Objects.requireNonNull(tags.get(key)))).orElseGet(()->Set.copyOf(structure.biomes().stream().toList()));
    var ctx=new Structure.GenerationContext(null,generator,generator.getBiomeSource(),climate,random,null,seed,new ChunkPos(x,z),accessor,allowed::contains);
    var stub=structure.findValidGenerationPoint(ctx);String prefix="P "+id+" "+x+" "+z+" ";
    if(stub.isEmpty()){out.println(prefix+"X");continue;}
    var s=stub.get();var piece=s.getPiecesBuilder().build().pieces().get(0);var box=piece.getBoundingBox();var pos=s.position();
    out.printf("%sS %d %d %d %s %d %d %d %d %d %d%n",prefix,pos.getX(),pos.getY(),pos.getZ(),piece.getOrientation()==null?"NONE":piece.getOrientation().name(),box.minX(),box.minY(),box.minZ(),box.maxX(),box.maxY(),box.maxZ());
   }
  }
 }
}
