import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.damagesource.DamageSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.storage.*;

public class GeneratedEntityRecordOracle {
    static class MockEntity extends Entity {
        int marker; boolean passenger; boolean write; boolean result;
        MockEntity() { super(null,null); }
        protected void defineSynchedData(SynchedEntityData.Builder builder) {}
        protected void readAdditionalSaveData(ValueInput input) {}
        protected void addAdditionalSaveData(ValueOutput output) {}
        public boolean hurtServer(ServerLevel level, DamageSource source, float damage) { return false; }
        public boolean isPassenger() { return passenger; }
        public boolean save(ValueOutput output) { if(write) output.putInt("n",marker); return result; }
        public RegistryAccess registryAccess() { return RegistryAccess.EMPTY; }
        public ProblemReporter.PathElement problemPath() { return () -> "mock"; }
    }
    public static void main(String[] args) throws Exception {
        var output=System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var biome=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));
        var ids=new IdMapper<Holder<Biome>>(); ids.add(biome);
        var factory=new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),Blocks.AIR.defaultBlockState(),null,Strategy.createForBiomes(ids),biome,null);
        var chunks=new HashMap<ChunkPos,ProtoChunk>();
        for(int z=-1;z<=1;z++) for(int x=-1;x<=1;x++) chunks.put(new ChunkPos(x,z),new ProtoChunk(new ChunkPos(x,z),UpgradeData.EMPTY,LevelHeightAccessor.create(0,16),factory,null));
        int[][] operations={{0,0,1,0,1,1},{-1,-1,2,0,1,1},{15,15,3,1,1,1},{15,15,4,0,0,0},{0,0,1,0,1,0}};
        for(var op:operations) {
            var entity=(MockEntity)unsafe.allocateInstance(MockEntity.class);
            entity.marker=op[2]; entity.passenger=op[3]!=0; entity.write=op[4]!=0; entity.result=op[5]!=0;
            var chunk=chunks.get(new ChunkPos(SectionPos.blockToSectionCoord(op[0]),SectionPos.blockToSectionCoord(op[1])));
            chunk.addEntity(entity);
        }
        for(var p:new int[][]{{0,0},{-1,-1},{1,0}}) {
            var values=new ArrayList<String>();
            for(var tag:chunks.get(new ChunkPos(p[0],p[1])).getEntities()) values.add(tag.getInt("n").map(Object::toString).orElse("empty"));
            output.println("E "+p[0]+" "+p[1]+" "+String.join(",",values));
        }
    }
}
