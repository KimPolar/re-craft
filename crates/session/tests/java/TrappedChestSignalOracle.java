import com.google.gson.*;
import java.nio.file.*;
import java.io.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.SignalGetter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.ChestBlock;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.block.entity.ContainerOpenersCounter;
import net.minecraft.world.level.block.entity.TrappedChestBlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.ChestType;
import net.minecraft.world.level.material.FluidState;

public class TrappedChestSignalOracle {
    static class World implements SignalGetter {
        BlockState state; BlockEntity entity;
        public BlockState getBlockState(BlockPos p) { return p.equals(BlockPos.ZERO) ? state : Blocks.AIR.defaultBlockState(); }
        public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
        public BlockEntity getBlockEntity(BlockPos p) { return p.equals(BlockPos.ZERO) ? entity : null; }
        public int getHeight() { return 384; }
        public int getMinY() { return -64; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var counterField=ChestBlockEntity.class.getDeclaredField("openersCounter"); counterField.setAccessible(true);
        var countField=ContainerOpenersCounter.class.getDeclaredField("openCount"); countField.setAccessible(true);
        var world=new World(); var rows=new JsonArray();
        for (boolean trapped : new boolean[]{false,true})
        for (var type : new ChestType[]{ChestType.SINGLE, ChestType.LEFT, ChestType.RIGHT})
            for (var facing : new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST})
                for (boolean water : new boolean[]{false,true})
                    for (int count : new int[]{0,1,7,15,20}) {
                        var state=(trapped ? Blocks.TRAPPED_CHEST : Blocks.CHEST).defaultBlockState().setValue(ChestBlock.TYPE,type).setValue(ChestBlock.FACING,facing).setValue(ChestBlock.WATERLOGGED,water);
                        var entity=trapped ? new TrappedChestBlockEntity(BlockPos.ZERO,state) : new ChestBlockEntity(BlockPos.ZERO,state);
                        countField.setInt(counterField.get(entity),count);
                        world.state=state; world.entity=entity;
                        for (var direction : new Direction[]{Direction.DOWN,Direction.UP,Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST}) {
                            var row=new JsonObject(); row.addProperty("trapped",trapped); row.addProperty("type",type.getSerializedName()); row.addProperty("facing",facing.getName()); row.addProperty("waterlogged",water); row.addProperty("count",count); row.addProperty("direction",direction.getName());
                            row.addProperty("weak",state.getSignal(world,BlockPos.ZERO,direction)); row.addProperty("direct",state.getDirectSignal(world,BlockPos.ZERO,direction)); row.addProperty("conductor",state.isRedstoneConductor(world,BlockPos.ZERO)); rows.add(row);
                            row.addProperty("entityId",BuiltInRegistries.BLOCK_ENTITY_TYPE.getKey(entity.getType()).toString());
                        }
                    }
        Files.writeString(Path.of(args[0]),rows.toString());
        var lookup=net.minecraft.data.registries.VanillaRegistries.createWorldLookup();
        var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(lookup);
        for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var saved=new TrappedChestBlockEntity(BlockPos.ZERO,Blocks.TRAPPED_CHEST.defaultBlockState());
        saved.setItem(4,new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.DIAMOND,3));
        var tag=saved.saveWithFullMetadata(registries);
        try(var out=new DataOutputStream(Files.newOutputStream(Path.of(args[0]+".nbt")))) { net.minecraft.nbt.NbtIo.write(tag,(DataOutput)out); }
    }
}
