import com.mojang.serialization.Codec;
import com.mojang.serialization.Lifecycle;
import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.storage.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pieces.*;

public class GeneratedStructureRegionOracle {
    static boolean sameLight(DataLayer first,DataLayer second) {
        return first==null ? second==null : second!=null && Arrays.equals(first.getData(),second.getData());
    }
    public static void main(String[] args)throws Exception {
        SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var directory=Path.of(args[0]);int x=Integer.parseInt(args[1]),z=Integer.parseInt(args[2]);
        var pos=new ChunkPos(x,z);
        CompoundTag chunk;
        try(var region=new RegionFile(new RegionStorageInfo("verification",null,"region"),
                directory.resolve("r."+Math.floorDiv(x,32)+"."+Math.floorDiv(z,32)+".mca"),
                directory,RegionFileVersion.VERSION_DEFLATE,false);
            var input=region.getChunkDataInputStream(pos)) {
            if(input==null)throw new AssertionError("generated chunk is missing");
            chunk=NbtIo.read((DataInput)input);
        }
        var expected=NbtIo.read(directory.resolve("expected.nbt"));
        if(!chunk.equals(expected))throw new AssertionError("region content differs from draft");
        if(chunk.getIntOr("DataVersion",0)!=5023)throw new AssertionError("incorrect data version");

        var lookup=VanillaRegistries.createWorldLookup();
        Structure structure=lookup.lookupOrThrow(Registries.STRUCTURE)
            .getOrThrow(ResourceKey.create(Registries.STRUCTURE,Identifier.parse("minecraft:buried_treasure"))).value();
        var registry=new MappedRegistry<Structure>(Registries.STRUCTURE,Lifecycle.stable());
        Registry.register(registry,Identifier.parse("example:treasure"),structure);registry.freeze();
        var access=new RegistryAccess.ImmutableRegistryAccess(List.of(registry));
        var context=new StructurePieceSerializationContext(null,access,null);
        var readStarts=SerializableChunkData.class.getDeclaredMethod("unpackStructureStart",StructurePieceSerializationContext.class,CompoundTag.class,long.class);
        var readReferences=SerializableChunkData.class.getDeclaredMethod("unpackStructureReferences",RegistryAccess.class,ChunkPos.class,CompoundTag.class);
        readStarts.setAccessible(true);readReferences.setAccessible(true);
        var structures=chunk.getCompoundOrEmpty("structures");
        var starts=(Map<?,?>)readStarts.invoke(null,context,structures,42L);
        var refs=(Map<?,?>)readReferences.invoke(null,access,pos,structures);
        var start=(StructureStart)starts.get(structure);
        if(start==null || !start.getChunkPos().equals(pos) || start.getPieces().size()!=1)
            throw new AssertionError("buried treasure start was not decoded");
        var box=start.getPieces().get(0).getBoundingBox();
        int bx=Integer.parseInt(args[3]),by=Integer.parseInt(args[4]),bz=Integer.parseInt(args[5]);
        if(box.minX()!=bx || box.minY()!=by || box.minZ()!=bz || box.maxX()!=bx || box.maxY()!=by || box.maxZ()!=bz)
            throw new AssertionError("buried treasure position differs");
        var set=(it.unimi.dsi.fastutil.longs.LongSet)refs.get(structure);
        if(set==null || !set.contains(ChunkPos.pack(x,z)))throw new AssertionError("own structure reference missing");

        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);
        var biome=Holder.direct((Biome)((sun.misc.Unsafe)field.get(null)).allocateInstance(Biome.class));
        var ids=new IdMapper<Holder<Biome>>();ids.add(biome);
        var blockStrategy=Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
        var biomeStrategy=Strategy.createForBiomes(ids);
        Codec<Holder<Biome>> biomeCodec=Codec.STRING.xmap(name->{
            if(!name.equals("example:test"))throw new IllegalArgumentException(name);
            return biome;
        },value->"example:test");
        var factory=new PalettedContainerFactory(blockStrategy,Blocks.AIR.defaultBlockState(),
            PalettedContainer.codecRW(BlockState.CODEC,blockStrategy,Blocks.AIR.defaultBlockState()),
            biomeStrategy,biome,PalettedContainer.codecRO(biomeCodec,biomeStrategy,biome));
        var height=LevelHeightAccessor.create(80,16);
        var parsed=SerializableChunkData.parse(height,factory,chunk);
        if(parsed==null)throw new AssertionError("chunk parse failed");
        var section=parsed.sectionData().stream().filter(s->s.y()==5).findFirst()
            .orElseThrow(()->new AssertionError("height-80 section missing: "+parsed.sectionData().size()));
        if(section.chunkSection().getBlockState(Math.floorMod(bx,16),Math.floorMod(by,16),Math.floorMod(bz,16)).getBlock()!=Blocks.CHEST)
            throw new AssertionError("treasure chest block missing");
        var output=parsed.write();var reread=SerializableChunkData.parse(height,factory,output);
        if(reread==null)throw new AssertionError("rewritten chunk parse failed");
        var other=reread.sectionData().stream().filter(s->s.y()==5).findFirst()
            .orElseThrow(()->new AssertionError("rewritten height-80 section missing"));
        for(int y=0;y<16;y++)for(int zz=0;zz<16;zz++)for(int xx=0;xx<16;xx++)
            if(section.chunkSection().getBlockState(xx,y,zz)!=other.chunkSection().getBlockState(xx,y,zz))
                throw new AssertionError("block state changed on Java round trip");
        for(int y=0;y<4;y++)for(int zz=0;zz<4;zz++)for(int xx=0;xx<4;xx++)
            if(section.chunkSection().getNoiseBiome(xx,y,zz)!=biome || other.chunkSection().getNoiseBiome(xx,y,zz)!=biome)
                throw new AssertionError("biome changed on Java round trip");
        if(parsed.sectionData().size()!=reread.sectionData().size())throw new AssertionError("section count changed");
        for(var original:parsed.sectionData()) {
            var copy=reread.sectionData().stream().filter(s->s.y()==original.y()).findFirst()
                .orElseThrow(()->new AssertionError("light section disappeared"));
            if(!sameLight(original.blockLight(),copy.blockLight()) || !sameLight(original.skyLight(),copy.skyLight()))
                throw new AssertionError("light changed on Java round trip at section "+original.y());
        }
        chunk.remove("sections");output.remove("sections");
        if(!chunk.equals(output))throw new AssertionError("chunk metadata changed on Java round trip: "+chunk+" / "+output);
    }
}
