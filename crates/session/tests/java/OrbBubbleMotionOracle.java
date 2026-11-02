import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.tags.TagKey;
import net.minecraft.world.entity.*;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.*;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;

public class OrbBubbleMotionOracle {
    static final Field POSITION;
    static { try { POSITION=Entity.class.getDeclaredField("position"); POSITION.setAccessible(true); } catch(Exception e) { throw new RuntimeException(e); } }
    static class World extends ClientLevel {
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public boolean isClientSide() { return true; }
        @Override public boolean noCollision(AABB bounds) { return true; }
        @Override public net.minecraft.world.entity.player.Player getNearestPlayer(Entity entity,double radius) { return null; }
        @Override public BlockState getBlockState(BlockPos pos) { return Blocks.STONE.defaultBlockState(); }
        @Override public FluidState getFluidState(BlockPos pos) { return Fluids.EMPTY.defaultFluidState(); }
    }
    static class Orb extends ExperienceOrb {
        Vec3 velocity; Level world; boolean surface,drag,floor,grounded; int calls;
        Orb() { super(null,0,0,0,7); }
        void pos(Vec3 value) { try { POSITION.set(this,value); } catch(Exception e) { throw new RuntimeException(e); } }
        @Override public Level level() { return world; }
        @Override public void baseTick() { resetFallDistance(); }
        @Override public boolean isEyeInFluid(TagKey<Fluid> tag) { return true; }
        @Override public boolean isNoGravity() { return false; }
        @Override public boolean onGround() { return grounded; }
        @Override public Vec3 getDeltaMovement() { return velocity; }
        @Override public void setDeltaMovement(Vec3 value) { velocity=value; }
        @Override public void setDeltaMovement(double x,double y,double z) { velocity=new Vec3(x,y,z); }
        @Override public MoveSimulationType getMoveSimulationType() { return MoveSimulationType.SERVER_AND_CLIENT; }
        @Override protected boolean updateFluidInteraction() { return false; }
        @Override public BlockPos blockPosition() { return new BlockPos(8,10,8); }
        @Override public BlockPos getBlockPosBelowThatAffectsMyMovement() { return new BlockPos(8,9,8); }
        @Override public void move(MoverType type,Vec3 requested) {
            double dy=requested.y;
            if(floor && getY()+dy<10.0) dy=10.0-getY();
            grounded=dy!=requested.y && requested.y<0.0; verticalCollisionBelow=grounded;
            pos(new Vec3(getX()+requested.x,getY()+dy,getZ()+requested.z));
            if(grounded) { resetFallDistance(); velocity=new Vec3(velocity.x,0.0,velocity.z); }
        }
        @Override protected void applyEffectsFromBlocks() { calls++; if(surface) onAboveBubbleColumn(drag,BlockPos.ZERO); else onInsideBubbleColumn(drag); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var level=Entity.class.getDeclaredField("level");
        var rows=new JsonArray();
        for(boolean surface:new boolean[]{false,true}) for(boolean drag:new boolean[]{false,true}) for(boolean floor:new boolean[]{false,true}) for(double initial:new double[]{-2,-0.9,-0.3,0,0.7,1.8,2}) {
            var orb=(Orb)unsafe.allocateInstance(Orb.class); orb.world=world; unsafe.putObject(orb,unsafe.objectFieldOffset(level),world);
            orb.surface=surface; orb.drag=drag; orb.floor=floor; orb.velocity=new Vec3(0.2,initial,-0.3); orb.pos(new Vec3(8.5,10.1,8.5)); orb.fallDistance=5;
            orb.setBoundingBox(new AABB(8.25,10.1,8.25,8.75,10.6,8.75));
            orb.tick();
            var row=new JsonObject(); row.addProperty("surface",surface); row.addProperty("drag",drag); row.addProperty("floor",floor); row.addProperty("initial",initial);
            row.addProperty("x",orb.getX()); row.addProperty("y",orb.getY()); row.addProperty("z",orb.getZ());
            row.addProperty("vx",orb.velocity.x); row.addProperty("vy",orb.velocity.y); row.addProperty("vz",orb.velocity.z); row.addProperty("fall",orb.fallDistance); row.addProperty("ground",orb.grounded); row.addProperty("calls",orb.calls); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
