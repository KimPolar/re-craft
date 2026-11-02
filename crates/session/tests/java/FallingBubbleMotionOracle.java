import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.phys.Vec3;

public class FallingBubbleMotionOracle {
    static final Field POSITION;
    static { try { POSITION=Entity.class.getDeclaredField("position"); POSITION.setAccessible(true); } catch(Exception e) { throw new RuntimeException(e); } }
    static class World extends ClientLevel { World() { super(null,null,null,null,0,0,null,false,0L,0); } }
    static class Falling extends FallingBlockEntity {
        Vec3 velocity; Level world; boolean surface,drag,floor,noGravity,grounded; int calls;
        Falling() { super(null,null); }
        void pos(Vec3 value) { try { POSITION.set(this,value); } catch(Exception e) { throw new RuntimeException(e); } }
        @Override public Level level() { return world; }
        @Override public boolean isNoGravity() { return noGravity; }
        @Override public boolean onGround() { return grounded; }
        @Override public Vec3 getDeltaMovement() { return velocity; }
        @Override public void setDeltaMovement(Vec3 value) { velocity=value; }
        @Override public void setDeltaMovement(double x,double y,double z) { velocity=new Vec3(x,y,z); }
        @Override public void move(MoverType type,Vec3 requested) {
            double dy=requested.y;
            if(floor && getY()+dy<10.0) dy=10.0-getY();
            grounded=dy!=requested.y && requested.y<0.0;
            pos(new Vec3(getX()+requested.x,getY()+dy,getZ()+requested.z));
            if(grounded) velocity=new Vec3(velocity.x,0.0,velocity.z);
        }
        @Override protected void applyEffectsFromBlocks() { calls++; if(surface) onAboveBubbleColumn(drag,BlockPos.ZERO); else onInsideBubbleColumn(drag); }
        @Override protected void handlePortal() { }
        @Override public void setRequiresPrecisePosition(boolean value) { }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class); var level=Entity.class.getDeclaredField("level");
        var block=FallingBlockEntity.class.getDeclaredField("blockState"); block.setAccessible(true);
        var rows=new JsonArray();
        for(boolean surface:new boolean[]{false,true}) for(boolean drag:new boolean[]{false,true}) for(boolean floor:new boolean[]{false,true}) for(boolean noGravity:new boolean[]{false,true}) for(double initial:new double[]{-2,-0.9,-0.3,0,0.7,1.8,2}) {
            var entity=(Falling)unsafe.allocateInstance(Falling.class); entity.world=world; unsafe.putObject(entity,unsafe.objectFieldOffset(level),world);
            entity.surface=surface; entity.drag=drag; entity.floor=floor; entity.noGravity=noGravity; entity.velocity=new Vec3(0.2,initial,-0.3); entity.pos(new Vec3(8.5,10.1,8.5)); entity.fallDistance=5;
            block.set(entity,Blocks.SAND.defaultBlockState()); entity.tick();
            var row=new JsonObject(); row.addProperty("surface",surface); row.addProperty("drag",drag); row.addProperty("floor",floor); row.addProperty("noGravity",noGravity); row.addProperty("initial",initial);
            row.addProperty("x",entity.getX()); row.addProperty("y",entity.getY()); row.addProperty("z",entity.getZ());
            row.addProperty("vx",entity.velocity.x); row.addProperty("vy",entity.velocity.y); row.addProperty("vz",entity.velocity.z); row.addProperty("fall",entity.fallDistance); row.addProperty("ground",entity.grounded); row.addProperty("calls",entity.calls); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
