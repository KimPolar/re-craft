import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import com.mojang.serialization.Lifecycle;
import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.jar.JarFile;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pools.*;
import net.minecraft.world.level.levelgen.structure.structures.JigsawStructure;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePieceSerializationContext;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class JigsawConfiguredCityOracle {
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var lookup=VanillaRegistries.createWorldLookup();
        var json=JsonParser.parseString(new String(JigsawConfiguredCityOracle.class.getClassLoader().getResourceAsStream("data/minecraft/worldgen/structure/ancient_city.json").readAllBytes(),StandardCharsets.UTF_8)).getAsJsonObject();
        json.addProperty("size",2);
        var structure=JigsawStructure.CODEC.codec().parse(lookup.createSerializationContext(JsonOps.INSTANCE),json).getOrThrow();
        Field uf=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); uf.setAccessible(true); var unsafe=(sun.misc.Unsafe)uf.get(null);
        var manager=(StructureTemplateManager)unsafe.allocateInstance(StructureTemplateManager.class);
        Field repository=StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var templates=new HashMap<Identifier,Optional<StructureTemplate>>();
        try(var jar=new JarFile(args[0])) {
            var entries=jar.entries();
            while(entries.hasMoreElements()) {
                var entry=entries.nextElement(); var name=entry.getName();
                if(!name.startsWith("data/minecraft/structure/ancient_city/") || !name.endsWith(".nbt")) continue;
                var id=Identifier.parse("minecraft:"+name.substring("data/minecraft/structure/".length(),name.length()-4));
                var template=new StructureTemplate();
                try(var stream=jar.getInputStream(entry)) { template.load(BuiltInRegistries.BLOCK,NbtIo.readCompressed(stream,NbtAccounter.unlimitedHeap())); }
                templates.put(id,Optional.of(template));
            }
        }
        repository.set(manager,templates);
        var registry=new MappedRegistry<StructureTemplatePool>(Registries.TEMPLATE_POOL,Lifecycle.stable());
        lookup.lookupOrThrow(Registries.TEMPLATE_POOL).listElements().forEach(holder->registry.register(holder.key(),holder.value(),RegistrationInfo.BUILT_IN)); registry.freeze();
        var processors=new MappedRegistry<StructureProcessorList>(Registries.PROCESSOR_LIST,Lifecycle.stable());
        lookup.lookupOrThrow(Registries.PROCESSOR_LIST).listElements().forEach(holder->processors.register(holder.key(),holder.value(),RegistrationInfo.BUILT_IN)); processors.freeze();
        var structures=new MappedRegistry<Structure>(Registries.STRUCTURE,Lifecycle.stable());
        structures.register(ResourceKey.create(Registries.STRUCTURE,Identifier.parse("minecraft:ancient_city")),structure,RegistrationInfo.BUILT_IN); structures.freeze();
        var delegate=new RegistryAccess.ImmutableRegistryAccess(List.of(registry,processors,structures));
        var vanillaLookup=lookup;
        RegistryAccess access=new RegistryAccess() {
            public <E> Optional<Registry<E>> lookup(ResourceKey<? extends Registry<? extends E>> key) { return delegate.lookup(key); }
            public java.util.stream.Stream<RegistryAccess.RegistryEntry<?>> registries() { return delegate.registries(); }
            public <T> RegistryOps<T> createSerializationContext(com.mojang.serialization.DynamicOps<T> ops) { return vanillaLookup.createSerializationContext(ops); }
        };
        var saveContext=new StructurePieceSerializationContext(null,access,manager);
        var generator=(JigsawPlacementOracle.FlatGenerator)unsafe.allocateInstance(JigsawPlacementOracle.FlatGenerator.class); generator.height=-27;
        var heights=LevelHeightAccessor.create(-64,384);
        for(long seed:new long[]{0,42,-1}) for(int[] xy:new int[][]{{0,0},{-3,2}}) {
            var random=new WorldgenRandom(new LegacyRandomSource(0)); random.setLargeFeatureSeed(seed,xy[0],xy[1]);
            var context=new Structure.GenerationContext(access,generator,null,null,null,null,manager,random,seed,new ChunkPos(xy[0],xy[1]),heights,h->true);
            var stub=structure.findGenerationPoint(context).orElseThrow();
            var point=stub.position(); var container=stub.getPiecesBuilder().build(); var pieces=container.pieces();
            out.printf("C\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d%n",seed,xy[0],xy[1],point.getX(),point.getY(),point.getZ(),pieces.size(),random.nextLong());
            if(args.length>1) { String filename=seed+"_"+xy[0]+"_"+xy[1]+"_start.nbt"; var start=new StructureStart(structure,new ChunkPos(xy[0],xy[1]),0,container); NbtIo.write(start.createTag(saveContext,new ChunkPos(xy[0],xy[1])),java.nio.file.Path.of(args[1],filename)); out.printf("S\t%d\t%d\t%d\t%s%n",seed,xy[0],xy[1],filename); }
            int index=0;
            for(var entry:pieces) {
                var piece=(PoolElementStructurePiece)entry; var p=piece.getPosition(); var b=piece.getBoundingBox();
                if(args.length>1) { String filename=seed+"_"+xy[0]+"_"+xy[1]+"_"+index+".nbt"; NbtIo.write(piece.createTag(saveContext),java.nio.file.Path.of(args[1],filename)); out.printf("T\t%d\t%d\t%d\t%d\t%s%n",seed,xy[0],xy[1],index,filename); }
                index++;
                out.printf("P\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d%n",piece.getRotation().ordinal(),p.getX(),p.getY(),p.getZ(),b.minX(),b.minY(),b.minZ(),b.maxX(),b.maxY(),b.maxZ(),piece.getGroundLevelDelta(),piece.getJunctions().size());
                for(var j:piece.getJunctions()) out.printf("J\t%d\t%d\t%d\t%d\t%s%n",j.getSourceX(),j.getSourceGroundY(),j.getSourceZ(),j.getDeltaY(),j.getDestProjection().getSerializedName());
            }
        }
    }
}
