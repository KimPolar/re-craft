import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.ai.village.poi.PoiTypes;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biomes;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.chunk.*;

public class PotentSulfurOracle {
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registries=VanillaRegistries.createWorldLookup();
        var biome=registries.lookupOrThrow(Registries.BIOME).getOrThrow(Biomes.PLAINS);
        var ids=new IdMapper<Holder<net.minecraft.world.level.biome.Biome>>();ids.add(biome);
        var palette=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),biome,null);
        var chunk=new ProtoChunk(new ChunkPos(0,0),UpgradeData.EMPTY,LevelHeightAccessor.create(0,16),palette,null);
        var empty=(LevelReader)java.lang.reflect.Proxy.newProxyInstance(LevelReader.class.getClassLoader(),new Class[]{LevelReader.class},(o,m,a)->{throw new AssertionError("unexpected survival query "+m);});
        int index=0;
        for(var state:Blocks.POTENT_SULFUR.getStateDefinition().getPossibleStates()) {
            var pos=new BlockPos(index,8,0); chunk.setBlockState(pos,state,2);
            var dummy=new CompoundTag();dummy.putString("id","DUMMY");dummy.putInt("x",pos.getX());dummy.putInt("y",pos.getY());dummy.putInt("z",pos.getZ());
            chunk.setBlockEntityNbt(dummy);
            if(chunk.getBlockEntity(pos)!=null || !dummy.equals(chunk.getBlockEntityNbtForSaving(pos,registries)))throw new AssertionError("placeholder changed");
            var entity=((EntityBlock)Blocks.POTENT_SULFUR).newBlockEntity(pos,state);
            if(entity==null)throw new AssertionError("missing block entity");
            chunk.setBlockEntity(entity);
            NbtIo.write(chunk.getBlockEntityNbtForSaving(pos,registries),Path.of(args[0]).resolve(index+".nbt"));
            out.printf("P %d %s %s %s %s %s %s %s%n",index,state.getValue(PotentSulfurBlock.STATE).getSerializedName(),state.canSurvive(empty,pos),state.isSolid(),state.liquid(),state.hasBlockEntity(),PoiTypes.hasPoi(state),state.getPostProcessPos(null,pos));
            index++;
        }
    }
}
