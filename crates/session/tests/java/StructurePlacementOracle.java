import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.level.block.state.properties.WallSide;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class StructurePlacementOracle {
    static String key(BlockState state) {
        var values = new TreeMap<String,String>();
        state.getValues().forEach(e -> values.put(e.property().getName(),e.valueName()));
        var out = new StringBuilder(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()).append('|');
        for (var e:values.entrySet()) out.append(e.getKey()).append('=').append(e.getValue()).append(';');
        return out.toString();
    }
    static ListTag ints(int... values) { var list = new ListTag(); for (int v:values) list.add(IntTag.valueOf(v)); return list; }
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class); bind.setAccessible(true);
        for (var block : List.of(Blocks.STONE_BRICK_WALL, Blocks.MOSSY_STONE_BRICK_WALL))
            bind.invoke(block.builtInRegistryHolder(), List.of(BlockTags.WALLS));
        for (var b : List.of(Blocks.STONE, Blocks.STONE_BRICKS, Blocks.MOSSY_STONE_BRICKS,
                Blocks.CRACKED_STONE_BRICKS, Blocks.CHISELED_STONE_BRICKS, Blocks.COBBLESTONE,
                Blocks.MOSSY_COBBLESTONE, Blocks.OBSIDIAN, Blocks.CRYING_OBSIDIAN, Blocks.GOLD_BLOCK,
                Blocks.NETHERRACK, Blocks.MAGMA_BLOCK, Blocks.BLACKSTONE, Blocks.POLISHED_BLACKSTONE,
                Blocks.POLISHED_BLACKSTONE_BRICKS, Blocks.CRACKED_POLISHED_BLACKSTONE_BRICKS,
                Blocks.CHISELED_POLISHED_BLACKSTONE, Blocks.IRON_BARS, Blocks.JUNGLE_LEAVES,
                Blocks.VINE, Blocks.CHEST, Blocks.AIR, Blocks.CAVE_AIR, Blocks.VOID_AIR,
                Blocks.STONE_BRICK_STAIRS, Blocks.MOSSY_STONE_BRICK_STAIRS, Blocks.STONE_STAIRS,
                Blocks.COBBLESTONE_STAIRS, Blocks.MOSSY_COBBLESTONE_STAIRS, Blocks.BLACKSTONE_STAIRS,
                Blocks.POLISHED_BLACKSTONE_STAIRS, Blocks.POLISHED_BLACKSTONE_BRICK_STAIRS,
                Blocks.STONE_BRICK_WALL, Blocks.MOSSY_STONE_BRICK_WALL,
                Blocks.COBBLESTONE_WALL, Blocks.MOSSY_COBBLESTONE_WALL,
                Blocks.BLACKSTONE_WALL, Blocks.POLISHED_BLACKSTONE_WALL, Blocks.POLISHED_BLACKSTONE_BRICK_WALL,
                Blocks.STONE_SLAB, Blocks.STONE_BRICK_SLAB, Blocks.MOSSY_STONE_BRICK_SLAB,
                Blocks.COBBLESTONE_SLAB, Blocks.MOSSY_COBBLESTONE_SLAB, Blocks.SMOOTH_STONE_SLAB,
                Blocks.BLACKSTONE_SLAB, Blocks.POLISHED_BLACKSTONE_SLAB, Blocks.POLISHED_BLACKSTONE_BRICK_SLAB))
            for (var state : b.getStateDefinition().getPossibleStates()) {
                var actual = state.getPostProcessPos(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
                var expected = b == Blocks.MAGMA_BLOCK ? BlockPos.ZERO.above() : null;
                if (!Objects.equals(actual, expected))
                    throw new AssertionError("unexpected postprocess state " + state + " -> " + actual);
            }
        var root=new CompoundTag(); root.put("size",ints(4,2,4));
        var palette=new ListTag(); palette.add(NbtUtils.writeBlockState(Blocks.STONE.defaultBlockState()));
        palette.add(NbtUtils.writeBlockState(Blocks.IRON_BARS.defaultBlockState()));
        palette.add(NbtUtils.writeBlockState(Blocks.STONE_BRICK_STAIRS.defaultBlockState()));
        palette.add(NbtUtils.writeBlockState(Blocks.STONE_BRICK_STAIRS.defaultBlockState().setValue(StairBlock.FACING, Direction.EAST)));
        palette.add(NbtUtils.writeBlockState(Blocks.STONE_SLAB.defaultBlockState())); root.put("palette",palette);
        var blocks=new ListTag();
        for (int[] b:new int[][]{{0,0,0,0},{2,0,0,0},{1,0,0,1},{1,0,1,1},{3,1,1,1},
                {2,1,1,2},{2,1,0,3},{0,1,1,4}}) {
            var tag=new CompoundTag(); tag.put("pos",ints(b[0],b[1],b[2])); tag.putInt("state",b[3]); blocks.add(tag);
        }
        root.put("blocks",blocks); root.put("entities",new ListTag());
        var template=new StructureTemplate(); template.load(BuiltInRegistries.BLOCK,root);
        var origin=new BlockPos(4,12,-6); var pivot=new BlockPos(2,0,2);
        for (var mirror:Mirror.values()) for (var rotation:Rotation.values()) {
            var states=new HashMap<BlockPos,BlockState>(); var events=new ArrayList<String>();
            var levelRandom=RandomSource.create(17L);
            var world=(ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},
                (proxy,m,a)->switch(m.getName()) {
                    case "getBlockState" -> states.getOrDefault((BlockPos)a[0],Blocks.AIR.defaultBlockState());
                    case "getFluidState" -> states.getOrDefault((BlockPos)a[0],Blocks.AIR.defaultBlockState()).getFluidState();
                    case "getRandom" -> levelRandom;
                    case "setBlock" -> { var p=((BlockPos)a[0]).immutable(); var s=(BlockState)a[1];
                        states.put(p,s); events.add("W\t"+p.getX()+"\t"+p.getY()+"\t"+p.getZ()+"\t"+key(s)+"\t"+a[2]); yield true; }
                    case "updateNeighborsAt" -> {var p=(BlockPos)a[0]; events.add("N\t"+p.getX()+"\t"+p.getY()+"\t"+p.getZ()); yield null;}
                    case "getBlockEntity" -> null;
                    default -> throw new UnsupportedOperationException(m.toString());
                });
            if (mirror == Mirror.NONE && rotation == Rotation.NONE) {
                for (var b : List.of(Blocks.STONE, Blocks.STONE_BRICKS, Blocks.MOSSY_STONE_BRICKS,
                        Blocks.CRACKED_STONE_BRICKS, Blocks.CHISELED_STONE_BRICKS, Blocks.COBBLESTONE,
                        Blocks.MOSSY_COBBLESTONE, Blocks.OBSIDIAN, Blocks.CRYING_OBSIDIAN, Blocks.GOLD_BLOCK,
                        Blocks.NETHERRACK, Blocks.MAGMA_BLOCK, Blocks.BLACKSTONE, Blocks.POLISHED_BLACKSTONE,
                        Blocks.POLISHED_BLACKSTONE_BRICKS, Blocks.CRACKED_POLISHED_BLACKSTONE_BRICKS,
                        Blocks.CHISELED_POLISHED_BLACKSTONE)) {
                    var s = b.defaultBlockState();
                    for (var d : Direction.values())
                        if (s.updateShape(world, world, BlockPos.ZERO, d, BlockPos.ZERO.relative(d),
                                Blocks.AIR.defaultBlockState(), levelRandom) != s)
                            throw new AssertionError("unexpected shape change for " + s);
                }
                for (var facing : Direction.Plane.HORIZONTAL) {
                    var chest = Blocks.CHEST.defaultBlockState().setValue(ChestBlock.FACING, facing);
                    for (var d : Direction.values())
                        for (var neighbor : List.of(Blocks.AIR, Blocks.STONE, Blocks.IRON_BARS,
                                Blocks.STONE_BRICK_STAIRS, Blocks.STONE_SLAB))
                            if (chest.updateShape(world, world, BlockPos.ZERO, d, BlockPos.ZERO.relative(d),
                                    neighbor.defaultBlockState(), levelRandom) != chest)
                                throw new AssertionError("unexpected chest shape change " + facing + " " + d + " " + neighbor);
                }
                var bars = Blocks.IRON_BARS.defaultBlockState();
                for (var d : Direction.Plane.HORIZONTAL)
                    if (bars.updateShape(world, world, BlockPos.ZERO, d, BlockPos.ZERO.relative(d),
                            Blocks.CHEST.defaultBlockState(), levelRandom) != bars)
                        throw new AssertionError("bars unexpectedly connect to chest " + d);
                for (var d : Direction.Plane.HORIZONTAL)
                    if (bars.updateShape(world, world, BlockPos.ZERO, d, BlockPos.ZERO.relative(d),
                            Blocks.STONE_BRICK_WALL.defaultBlockState(), levelRandom) == bars)
                        throw new AssertionError("bars failed to connect to wall " + d);
                var wall = Blocks.STONE_BRICK_WALL.defaultBlockState();
                var low = wall.updateShape(world, world, BlockPos.ZERO, Direction.EAST, BlockPos.ZERO.east(), wall, levelRandom);
                if (low.getValue(WallBlock.EAST) != WallSide.LOW || !low.getValue(WallBlock.UP))
                    throw new AssertionError("unexpected wall connection " + low);
                states.put(BlockPos.ZERO.above(), Blocks.STONE.defaultBlockState());
                var tall = wall.updateShape(world, world, BlockPos.ZERO, Direction.EAST, BlockPos.ZERO.east(), wall, levelRandom);
                if (tall.getValue(WallBlock.EAST) != WallSide.TALL || !tall.getValue(WallBlock.UP))
                    throw new AssertionError("unexpected wall with ceiling " + tall);
                states.clear();
                var air = Blocks.AIR.defaultBlockState();
                bind.invoke(Blocks.AIR.builtInRegistryHolder(), List.of(BlockTags.WALLS, BlockTags.WALL_POST_OVERRIDE));
                var tagged = wall.updateShape(world, world, BlockPos.ZERO, Direction.EAST, BlockPos.ZERO.east(), air, levelRandom);
                if (tagged.getValue(WallBlock.EAST) != WallSide.LOW) throw new AssertionError("wall tag ignored " + tagged);
                if (bars.updateShape(world, world, BlockPos.ZERO, Direction.EAST, BlockPos.ZERO.east(), air, levelRandom) == bars)
                    throw new AssertionError("bars ignored wall tag");
                var straight = wall.setValue(WallBlock.NORTH, WallSide.LOW).setValue(WallBlock.SOUTH, WallSide.LOW).setValue(WallBlock.UP, false);
                if (!straight.updateShape(world, world, BlockPos.ZERO, Direction.UP, BlockPos.ZERO.above(), air, levelRandom).getValue(WallBlock.UP))
                    throw new AssertionError("wall post override ignored");
                bind.invoke(Blocks.AIR.builtInRegistryHolder(), List.of());
                if (!events.isEmpty()) throw new AssertionError("unexpected shape effects");
            }
            var settings=new StructurePlaceSettings().setMirror(mirror).setRotation(rotation).setRotationPivot(pivot)
                .setBoundingBox(new BoundingBox(-20,-20,-20,20,30,20)).setIgnoreEntities(true);
            var random=new WorldgenRandom(new XoroshiroRandomSource(42));
            if (!template.placeInWorld(world,origin,origin,settings,random,2)) throw new AssertionError("empty template");
            out.printf("CASE\t%d\t%d\t%d%n",mirror.ordinal(),rotation.ordinal(),events.size());
            for (var e:events) out.println(e);
            out.printf("END\t%d%n",random.nextLong());
        }
    }
}
