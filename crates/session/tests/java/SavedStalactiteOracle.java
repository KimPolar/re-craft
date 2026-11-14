import java.io.*;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.Direction;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.nbt.*;
import net.minecraft.resources.RegistryOps;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SpeleothemBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.SpeleothemThickness;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.ValueInput;
import net.minecraft.world.phys.Vec3;

public class SavedStalactiteOracle {
    static class Falling extends FallingBlockEntity {
        Falling() { super(null,null); }
        void loadExtra(ValueInput input) { super.readAdditionalSaveData(input); }
    }
    static class World extends ClientLevel {
        BlockState replacement;
        Entity created;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public int getNextEntityId() { return 1; }
        @Override public boolean setBlock(BlockPos pos,BlockState state,int flags,int depth) { replacement=state;return true; }
        @Override public boolean addFreshEntity(Entity entity) { created=entity;return true; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        var registry=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registry))pending.apply();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        var hurt=FallingBlockEntity.class.getDeclaredField("hurtEntities");hurt.setAccessible(true);
        var amount=FallingBlockEntity.class.getDeclaredField("fallDamagePerDistance");amount.setAccessible(true);
        var maximum=FallingBlockEntity.class.getDeclaredField("fallDamageMax");maximum.setAccessible(true);
        int index=0;
        for(var block:new net.minecraft.world.level.block.Block[]{Blocks.POINTED_DRIPSTONE,Blocks.SULFUR_SPIKE})
            for(var thickness:new SpeleothemThickness[]{SpeleothemThickness.TIP,SpeleothemThickness.TIP_MERGE,SpeleothemThickness.FRUSTUM,SpeleothemThickness.MIDDLE,SpeleothemThickness.BASE}) {
                var state=block.defaultBlockState().setValue(SpeleothemBlock.TIP_DIRECTION,Direction.DOWN)
                    .setValue(SpeleothemBlock.THICKNESS,thickness).setValue(SpeleothemBlock.WATERLOGGED,false);
                boolean tip=thickness==SpeleothemThickness.TIP || thickness==SpeleothemThickness.TIP_MERGE;
                for(boolean wet:new boolean[]{false,true}) {
                    var source=state.setValue(SpeleothemBlock.WATERLOGGED,wet);
                    var entity=FallingBlockEntity.fall(world,new BlockPos(8,14,8),source);
                    if(entity.getBlockState()!=state || world.created!=entity || world.replacement.getBlock()!=(wet?Blocks.WATER:Blocks.AIR))
                        throw new AssertionError("fall creation differs at "+index+" wet="+wet);
                }
                Path path=Path.of(args[0],(args[1].equals("generate")?"java-":"rust-")+index+".nbt");
                if(args[1].equals("generate")) {
                    var tag=new CompoundTag();tag.putString("id","minecraft:falling_block");
                    var pos=new ListTag();for(double value:new double[]{8.5,14,8.5})pos.add(DoubleTag.valueOf(value));tag.put("Pos",pos);
                    tag.put("BlockState",BlockState.CODEC.encodeStart(RegistryOps.create(NbtOps.INSTANCE,registry),state).getOrThrow());
                    tag.putBoolean("HurtEntities",tip);tag.putFloat("FallHurtAmount",tip?7.0f:0.0f);tag.putInt("FallHurtMax",40);
                    NbtIo.write(tag,path);
                } else {
                    var input=new DataInputStream(Files.newInputStream(path));var tag=NbtIo.read((DataInput)input);input.close();
                    var value=TagValueInput.create(ProblemReporter.DISCARDING,registry,tag);
                    var entity=(Falling)unsafe.allocateInstance(Falling.class);entity.loadExtra(value);
                    if(entity.getBlockState()!=state || hurt.getBoolean(entity)!=tip || amount.getFloat(entity)!=(tip?7.0f:0.0f) || maximum.getInt(entity)!=40 ||
                       !value.read("Pos",Vec3.CODEC).orElseThrow().equals(new Vec3(8.5,14,8.5)))
                        throw new AssertionError("stalactite entity differs at "+index+": "+tag);
                }
                index++;
            }
    }
}
