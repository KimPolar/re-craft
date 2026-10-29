import java.util.List;
import java.util.function.Predicate;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.item.context.BlockPlaceContext;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.AABB;

public class SoilPlacementOracle {
    static class Probe extends Entity {
        boolean spectator;
        int calls;
        double rise;
        Probe() { super((EntityType<?>)null, null); }
        @Override public boolean isSpectator() { return spectator; }
        @Override public void teleportRelative(double x, double y, double z) {
            calls++; rise += y;
            setBoundingBox(getBoundingBox().move(x,y,z));
        }
        @Override protected void defineSynchedData(net.minecraft.network.syncher.SynchedEntityData.Builder b) {}
        @Override protected void readAdditionalSaveData(net.minecraft.world.level.storage.ValueInput input) {}
        @Override protected void addAdditionalSaveData(net.minecraft.world.level.storage.ValueOutput output) {}
        @Override public boolean hurtServer(net.minecraft.server.level.ServerLevel level, net.minecraft.world.damagesource.DamageSource source, float amount) { return false; }
    }
    static class World extends ClientLevel {
        BlockState cover;
        List<Entity> entities;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public BlockState getBlockState(BlockPos pos) { return cover; }
        @Override public List<Entity> getEntities(Entity except, AABB box, Predicate<? super Entity> predicate) {
            return entities.stream().filter(e -> e != except && box.intersects(e.getBoundingBox()) && predicate.test(e)).toList();
        }
    }
    static class Context extends BlockPlaceContext {
        Level world;
        Context() { super(null,null,null,null,null); }
        @Override public Level getLevel() { return world; }
        @Override public BlockPos getClickedPos() { return BlockPos.ZERO; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var context=(Context)unsafe.allocateInstance(Context.class); context.world=world;
        var normal=(Probe)unsafe.allocateInstance(Probe.class);
        var spectator=(Probe)unsafe.allocateInstance(Probe.class); spectator.spectator=true;
        var box=new AABB(0.3,0.9375,0.3,0.7,2.7375,0.7);
        normal.setBoundingBox(box); spectator.setBoundingBox(box);
        world.entities=List.of(normal,spectator);
        world.cover=Blocks.STONE.defaultBlockState();
        if (!Blocks.DIRT_PATH.getStateForPlacement(context).is(Blocks.DIRT)) throw new AssertionError("path did not become dirt");
        if (normal.calls!=1 || normal.rise!=0.0625 || spectator.calls!=0) throw new AssertionError("path push or spectator filtering differs");
        normal.calls=0; normal.rise=0; normal.setBoundingBox(box);
        if (!Blocks.FARMLAND.getStateForPlacement(context).is(Blocks.DIRT)) throw new AssertionError("farmland did not become dirt");
        if (normal.calls!=0 || spectator.calls!=0) throw new AssertionError("farmland unexpectedly pushed an entity");
        world.cover=Blocks.AIR.defaultBlockState();
        if (!Blocks.DIRT_PATH.getStateForPlacement(context).is(Blocks.DIRT_PATH)) throw new AssertionError("uncovered path changed");
        if (normal.calls!=0 || spectator.calls!=0) throw new AssertionError("uncovered path unexpectedly pushed an entity");
    }
}
