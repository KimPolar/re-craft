import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.decoration.ArmorStand;
import net.minecraft.world.item.enchantment.LevelBasedValue;
import net.minecraft.world.item.enchantment.effects.ReplaceDisk;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.blockpredicates.BlockPredicate;
import net.minecraft.world.level.levelgen.feature.stateproviders.BlockStateProvider;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.redstone.CollectingNeighborUpdater;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.VoxelShape;

public class FrostWalkerBubbleOracle {
    static class World extends IceBubbleTagOracle.World {
        World() { super(); }
        @Override public boolean isUnobstructed(Entity except,VoxelShape shape) { return true; }
        @Override public boolean setBlockAndUpdate(BlockPos pos,BlockState state) { return setBlock(pos,state,3,512); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class); world.attributes=EnvironmentAttributeSystem.builder().build();
        var entity=(ArmorStand)unsafe.allocateInstance(ArmorStand.class); var entityRandom=Entity.class.getDeclaredField("random");
        unsafe.putObject(entity,unsafe.objectFieldOffset(entityRandom),new LegacyRandomSource(0));
        var predicate=BlockPredicate.allOf(BlockPredicate.matchesBlocks(new Vec3i(0,1,0),List.of(Blocks.AIR,Blocks.CAVE_AIR,Blocks.VOID_AIR)),BlockPredicate.matchesBlocks(Blocks.WATER),BlockPredicate.matchesFluids(Fluids.WATER),BlockPredicate.unobstructed());
        var effect=new ReplaceDisk(LevelBasedValue.constant(3),LevelBasedValue.constant(1),new Vec3i(0,-1,0),Optional.of(predicate),BlockStateProvider.holderOf(Blocks.FROSTED_ICE),Optional.empty());
        var rows=new JsonArray();
        for(int tags=0;tags<4;tags++) for(boolean waterTagged:new boolean[]{false,true}) for(int seed=0;seed<4;seed++) {
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,Map.of(
                BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,(tags & 1)!=0?List.of(Blocks.STONE.builtInRegistryHolder()):List.of(),
                BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP,(tags & 2)!=0?List.of(Blocks.STONE.builtInRegistryHolder()):List.of()))).apply();
            BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID,Map.of(
                FluidTags.WATER,List.of(Fluids.WATER.builtInRegistryHolder(),Fluids.FLOWING_WATER.builtInRegistryHolder()),
                FluidTags.BUBBLE_COLUMN_CAN_OCCUPY,waterTagged?List.of(Fluids.WATER.builtInRegistryHolder()):List.of()))).apply();
            world.blocks=new HashMap<>(); world.random=new LegacyRandomSource(seed); world.bubble=false; world.updater=new CollectingNeighborUpdater(world,100000); world.blockTicks=new JsonArray(); world.fluidTicks=new JsonArray(); world.pending=new HashSet<>();
            for(int x=0;x<16;x++) for(int z=0;z<16;z++) world.blocks.put(new BlockPos(x,0,z),Blocks.STONE.defaultBlockState());
            for(int x=4;x<=12;x++) for(int z=4;z<=12;z++) world.blocks.put(new BlockPos(x,1,z),Blocks.WATER.defaultBlockState());
            world.blocks.put(new BlockPos(8,2,6),Blocks.STONE.defaultBlockState());
            effect.apply(world,1,null,entity,new Vec3(8.5,2.0,8.5));
            var row=new JsonObject(); row.addProperty("tags",tags); row.addProperty("water",waterTagged); row.addProperty("seed",seed); row.add("blockTicks",world.blockTicks); row.add("fluidTicks",world.fluidTicks); row.addProperty("next",world.random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
