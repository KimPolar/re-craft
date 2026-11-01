import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.util.RandomSource;
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
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.VoxelShape;

public class FrostWalkerPlacementOracle {
    static class World extends ServerLevel {
        Map<BlockPos,BlockState> blocks; RandomSource random; JsonArray scheduled;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public RandomSource getRandom() { return random; }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState()); }
        @Override public boolean isUnobstructed(Entity except, VoxelShape shape) { return true; }
        @Override public boolean setBlockAndUpdate(BlockPos pos, BlockState state) {
            var old = getBlockState(pos);
            if (old == state) return false;
            blocks.put(pos.immutable(), state);
            state.onPlace(this, pos, old, false);
            return true;
        }
        @Override public void scheduleTick(BlockPos pos, Block block, int delay) {
            JsonArray entry = new JsonArray(); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(delay); scheduled.add(entry);
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var world = (World)unsafe.allocateInstance(World.class);
        var entity = (ArmorStand)unsafe.allocateInstance(ArmorStand.class);
        var entityRandom = Entity.class.getDeclaredField("random");
        unsafe.putObject(entity, unsafe.objectFieldOffset(entityRandom), new LegacyRandomSource(0));
        var predicate = BlockPredicate.allOf(
            BlockPredicate.matchesBlocks(new Vec3i(0,1,0), List.of(Blocks.AIR, Blocks.CAVE_AIR, Blocks.VOID_AIR)),
            BlockPredicate.matchesBlocks(Blocks.WATER), BlockPredicate.matchesFluids(Fluids.WATER), BlockPredicate.unobstructed());
        var effect = new ReplaceDisk(LevelBasedValue.constant(3), LevelBasedValue.constant(1), new Vec3i(0,-1,0),
            Optional.of(predicate), BlockStateProvider.holderOf(Blocks.FROSTED_ICE), Optional.empty());
        var rows = new JsonArray();
        for (int seed=0; seed<32; seed++) {
            world.blocks = new HashMap<>(); world.scheduled = new JsonArray(); world.random = new LegacyRandomSource(seed);
            for (int x=0; x<16; x++) for (int z=0; z<16; z++) world.blocks.put(new BlockPos(x,0,z), Blocks.STONE.defaultBlockState());
            for (int x=4; x<=12; x++) for (int z=4; z<=12; z++) world.blocks.put(new BlockPos(x,1,z), Blocks.WATER.defaultBlockState());
            world.blocks.put(new BlockPos(8,2,6), Blocks.STONE.defaultBlockState());
            effect.apply(world, 1, null, entity, new Vec3(8.5,2.0,8.5));
            var row = new JsonObject(); row.addProperty("seed",seed); row.add("scheduled",world.scheduled); row.addProperty("next",world.random.nextInt()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
