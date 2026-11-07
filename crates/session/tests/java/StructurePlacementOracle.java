import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
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
        for (var b : List.of(Blocks.STONE, Blocks.STONE_BRICKS, Blocks.MOSSY_STONE_BRICKS,
                Blocks.CRACKED_STONE_BRICKS, Blocks.CHISELED_STONE_BRICKS, Blocks.COBBLESTONE,
                Blocks.MOSSY_COBBLESTONE, Blocks.OBSIDIAN, Blocks.CRYING_OBSIDIAN, Blocks.GOLD_BLOCK,
                Blocks.NETHERRACK, Blocks.MAGMA_BLOCK, Blocks.BLACKSTONE, Blocks.POLISHED_BLACKSTONE,
                Blocks.POLISHED_BLACKSTONE_BRICKS, Blocks.CRACKED_POLISHED_BLACKSTONE_BRICKS,
                Blocks.CHISELED_POLISHED_BLACKSTONE, Blocks.IRON_BARS, Blocks.JUNGLE_LEAVES,
                Blocks.VINE, Blocks.AIR, Blocks.CAVE_AIR, Blocks.VOID_AIR))
            for (var state : b.getStateDefinition().getPossibleStates()) {
                var actual = state.getPostProcessPos(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
                var expected = b == Blocks.MAGMA_BLOCK ? BlockPos.ZERO.above() : null;
                if (!Objects.equals(actual, expected))
                    throw new AssertionError("unexpected postprocess state " + state + " -> " + actual);
            }
        var root=new CompoundTag(); root.put("size",ints(4,2,4));
        var palette=new ListTag(); palette.add(NbtUtils.writeBlockState(Blocks.STONE.defaultBlockState()));
        palette.add(NbtUtils.writeBlockState(Blocks.IRON_BARS.defaultBlockState())); root.put("palette",palette);
        var blocks=new ListTag();
        for (int[] b:new int[][]{{0,0,0,0},{2,0,0,0},{1,0,0,1},{1,0,1,1},{3,1,1,1}}) {
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
