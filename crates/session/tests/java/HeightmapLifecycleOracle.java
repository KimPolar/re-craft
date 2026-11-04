import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.Heightmap;

public class HeightmapLifecycleOracle {
    static JsonObject capture(ProtoChunk chunk) {
        var result = new JsonObject();
        for(var entry:chunk.getHeightmaps()) result.addProperty(entry.getKey().getSerializedName(), entry.getValue().getFirstAvailable(0,0));
        return result;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind=Holder.Reference.class.getDeclaredMethod("bindTags",Collection.class); bind.setAccessible(true);
        bind.invoke(Blocks.STONE.builtInRegistryHolder(),List.of(BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP,BlockTags.BLOCKS_MOTION_IN_HEIGHTMAP_NO_LEAVES));
        bind.invoke(Blocks.ANCIENT_DEBRIS.builtInRegistryHolder(),List.of());
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var biome=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));
        var ids=new IdMapper<Holder<Biome>>(); ids.add(biome);
        var palette=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),biome,null);
        var chunk=new ProtoChunk(new ChunkPos(0,0),UpgradeData.EMPTY,LevelHeightAccessor.create(0,16),palette,null);
        chunk.setPersistedStatus(ChunkStatus.TERRAIN);
        for(int y=0;y<8;y++) for(int z=0;z<16;z++) for(int x=0;x<16;x++) chunk.getSection(0).setBlockState(x,y,z,Blocks.STONE.defaultBlockState(),false);
        chunk.getSection(0).setBlockState(1,7,0,Blocks.ANCIENT_DEBRIS.defaultBlockState(),false);
        Heightmap.primeHeightmaps(chunk,EnumSet.of(Heightmap.Types.OCEAN_FLOOR_WG,Heightmap.Types.WORLD_SURFACE_WG));
        var result=new JsonArray(); result.add(capture(chunk));
        chunk.setBlockState(new BlockPos(1,7,0),Blocks.ANCIENT_DEBRIS.defaultBlockState(),2); result.add(capture(chunk));
        chunk.getSection(0).setBlockState(0,12,0,Blocks.STONE.defaultBlockState(),false); result.add(capture(chunk));
        chunk.getHeight(Heightmap.Types.WORLD_SURFACE,0,0); result.add(capture(chunk));
        chunk.getSection(0).setBlockState(0,13,0,Blocks.STONE.defaultBlockState(),false); result.add(capture(chunk));
        chunk.getHeight(Heightmap.Types.OCEAN_FLOOR,0,0); result.add(capture(chunk));
        chunk.setBlockState(new BlockPos(0,7,0),Blocks.ANCIENT_DEBRIS.defaultBlockState(),2); result.add(capture(chunk));
        Files.writeString(Path.of(args[0]),result.toString());
    }
}
