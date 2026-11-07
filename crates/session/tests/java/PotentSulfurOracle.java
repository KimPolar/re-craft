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
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.feature.SimpleBlockFeature;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;

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
            for(var direction:Direction.values())
                if(!Block.isShapeFullBlock(state.getFaceOcclusionShape(direction)))
                    throw new AssertionError("potent sulfur face is not full: "+state+" "+direction);
            var pos=new BlockPos(index,8,0); chunk.setBlockState(pos,state,2);
            var dummy=new CompoundTag();dummy.putString("id","DUMMY");dummy.putInt("x",pos.getX());dummy.putInt("y",pos.getY());dummy.putInt("z",pos.getZ());
            chunk.setBlockEntityNbt(dummy);
            chunk.setBlockState(pos,Blocks.POTENT_SULFUR.defaultBlockState(),2);
            chunk.setBlockEntityNbt(dummy);
            chunk.setBlockState(pos,state,2);
            chunk.setBlockEntityNbt(dummy);
            if(chunk.getBlockEntity(pos)!=null || !dummy.equals(chunk.getBlockEntityNbtForSaving(pos,registries)))throw new AssertionError("placeholder changed");
            var entity=((EntityBlock)Blocks.POTENT_SULFUR).newBlockEntity(pos,state);
            if(entity==null)throw new AssertionError("missing block entity");
            chunk.setBlockEntity(entity);
            NbtIo.write(chunk.getBlockEntityNbtForSaving(pos,registries),Path.of(args[0]).resolve(index+".nbt"));
            out.printf("P %d %s %s %s %s %s %s %s%n",index,state.getValue(PotentSulfurBlock.STATE).getSerializedName(),state.canSurvive(empty,pos),state.isSolid(),state.liquid(),state.hasBlockEntity(),PoiTypes.hasPoi(state),state.getPostProcessPos(null,pos));
            // WorldGenRegion removes the old protochunk entity when a block
            // with an entity is replaced by one without an entity.
            var replacement=new BlockPos(index,7,1);
            chunk.setBlockState(replacement,state,2);
            var pending=new CompoundTag();pending.putString("id","DUMMY");pending.putInt("x",replacement.getX());pending.putInt("y",replacement.getY());pending.putInt("z",replacement.getZ());chunk.setBlockEntityNbt(pending);
            if(chunk.getBlockEntityNbt(replacement)==null)throw new AssertionError("missing pending sulfur");
            var magma=Blocks.MAGMA_BLOCK.defaultBlockState();
            var previous=chunk.setBlockState(replacement,magma,2);
            if(previous.hasBlockEntity() && !magma.hasBlockEntity())chunk.removeBlockEntity(replacement);
            if(chunk.getBlockEntityNbt(replacement)!=null || chunk.getBlockEntity(replacement)!=null || chunk.getBlockEntitiesPos().contains(replacement))throw new AssertionError("stale sulfur entity");
            index++;
        }
        for(var state:Blocks.POTENT_SULFUR.getStateDefinition().getPossibleStates())
        for(var oldBlock:new Block[]{Blocks.STONE,Blocks.SULFUR,Blocks.CINNABAR,Blocks.WATER})
        for(int restricted=0;restricted<2;restricted++)for(int scheduled=0;scheduled<2;scheduled++)for(long seed:new long[]{0,7}) {
            var pos=new BlockPos(8,7,8); chunk.removeBlockEntity(pos); chunk.setBlockState(pos,oldBlock.defaultBlockState(),2);
            int limit=restricted; var ticks=new java.util.ArrayList<String>();
            var level=(WorldGenLevel)java.lang.reflect.Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class[]{WorldGenLevel.class},(o,m,a)->{
                var p=(BlockPos)a[0];
                return switch(m.getName()) {
                    case "getBlockState" -> chunk.getBlockState(p);
                    case "setBlock" -> {if((int)a[2]!=2)throw new AssertionError("wrong flags");if(limit!=0)yield false;
                        var next=(net.minecraft.world.level.block.state.BlockState)a[1];chunk.setBlockState(p,next,2);
                        if(next.hasBlockEntity()){var tag=new CompoundTag();tag.putString("id","DUMMY");tag.putInt("x",p.getX());tag.putInt("y",p.getY());tag.putInt("z",p.getZ());chunk.setBlockEntityNbt(tag);} yield true;}
                    case "scheduleTick" -> {if((int)a[2]!=1)throw new AssertionError("wrong tick delay");ticks.add(net.minecraft.core.registries.BuiltInRegistries.BLOCK.getKey((Block)a[1]).toString());yield null;}
                    default -> throw new AssertionError("unexpected feature query "+m);
                };
            });
            var feature=new SimpleBlockFeature(Holder.direct(BlockStateProvider.of(state)),scheduled!=0);
            var random=new WorldgenRandom(new XoroshiroRandomSource(seed));
            boolean placed=feature.place(level,null,random,pos);
            var after=chunk.getBlockState(pos);var pending=chunk.getBlockEntityNbt(pos);
            if(ticks.size()!=(scheduled!=0?1:0))throw new AssertionError("wrong tick count");
            out.printf("F %s %s %d %d %d %s %s %s %s %d%n",state.getValue(PotentSulfurBlock.STATE).getSerializedName(),net.minecraft.core.registries.BuiltInRegistries.BLOCK.getKey(oldBlock),restricted,scheduled,seed,placed,net.minecraft.core.registries.BuiltInRegistries.BLOCK.getKey(after.getBlock()),pending!=null,ticks.isEmpty()?"-":ticks.getFirst(),random.nextLong());
        }
    }
}
