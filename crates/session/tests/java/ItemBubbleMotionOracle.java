import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.tags.TagKey;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.*;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.phys.Vec3;

public class ItemBubbleMotionOracle {
    static final Field POSITION;
    static { try { POSITION=Entity.class.getDeclaredField("position"); POSITION.setAccessible(true); } catch(Exception e) { throw new RuntimeException(e); } }
    static class World extends ClientLevel {
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public boolean isClientSide() { return true; }
        @Override public BlockState getBlockState(BlockPos pos) { return Blocks.AIR.defaultBlockState(); }
    }
    static class Item extends ItemEntity {
        Vec3 velocity; Level world; boolean surface,drag; int calls;
        Item() { super(null,0,0,0,ItemStack.EMPTY); }
        void pos(Vec3 value) { try { POSITION.set(this,value); } catch(Exception e) { throw new RuntimeException(e); } }
        @Override public Level level() { return world; }
        @Override public ItemStack getItem() { return new ItemStack(Items.DIRT); }
        @Override public void baseTick() { if(isInWater()) resetFallDistance(); }
        @Override public boolean isInWater() { return true; }
        @Override public boolean isInLava() { return false; }
        @Override public double getFluidHeight(TagKey<Fluid> tag) { return 1.0; }
        @Override public boolean isNoGravity() { return false; }
        @Override public boolean onGround() { return false; }
        @Override public int getId() { return 1; }
        @Override public Vec3 getDeltaMovement() { return velocity; }
        @Override public void setDeltaMovement(Vec3 value) { velocity=value; }
        @Override public MoveSimulationType getMoveSimulationType() { return MoveSimulationType.SERVER_AND_CLIENT; }
        @Override protected boolean updateFluidInteraction() { return false; }
        @Override public void move(MoverType type,Vec3 movement) { pos(new Vec3(getX()+movement.x,getY()+movement.y,getZ()+movement.z)); }
        @Override protected void applyEffectsFromBlocks() { calls++; if(surface) onAboveBubbleColumn(drag,BlockPos.ZERO); else onInsideBubbleColumn(drag); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Items.DIRT.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var level=Entity.class.getDeclaredField("level");
        var rows=new JsonArray();
        for(boolean surface:new boolean[]{false,true}) for(boolean drag:new boolean[]{false,true}) for(double initial:new double[]{-2,-0.9,-0.3,0,0.7,1.8,2}) {
            var item=(Item)unsafe.allocateInstance(Item.class); item.world=world; unsafe.putObject(item,unsafe.objectFieldOffset(level),world);
            item.surface=surface; item.drag=drag; item.velocity=new Vec3(0.2,initial,-0.3); item.pos(new Vec3(8.5,10,8.5)); item.fallDistance=5;
            item.tick();
            var row=new JsonObject(); row.addProperty("surface",surface); row.addProperty("drag",drag); row.addProperty("initial",initial);
            row.addProperty("x",item.getX()); row.addProperty("y",item.getY()); row.addProperty("z",item.getZ());
            row.addProperty("vx",item.velocity.x); row.addProperty("vy",item.velocity.y); row.addProperty("vz",item.velocity.z); row.addProperty("fall",item.fallDistance); row.addProperty("calls",item.calls); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
