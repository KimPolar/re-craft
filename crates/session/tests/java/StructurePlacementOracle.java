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
import net.minecraft.world.attribute.EnvironmentAttributeReader;
import net.minecraft.world.level.material.Fluid;
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
    static void markers(java.io.PrintStream out, String name, StructureTemplate template, BlockPos origin) {
        var size = template.getSize(); var pivot = new BlockPos(size.getX()/2,0,size.getZ()/2);
        for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) for (int c=0;c<2;c++) {
            var clip = c==0 ? new BoundingBox(-50,-20,-50,50,60,50) : new BoundingBox(2,10,-8,6,16,-4);
            var settings = new StructurePlaceSettings().setMirror(mirror).setRotation(rotation).setRotationPivot(pivot).setBoundingBox(clip);
            var found = template.filterBlocks(origin, settings, Blocks.JIGSAW).stream().filter(b -> b.nbt()!=null).toList();
            out.printf("MCASE\t%s\t%d\t%d\t%d\t%d%n",name,mirror.ordinal(),rotation.ordinal(),c,found.size());
            for (var marker : found) {
                var p=marker.pos(); var raw=marker.nbt().getStringOr("final_state","minecraft:air");
                var state=Blocks.AIR.defaultBlockState();
                try { state=net.minecraft.commands.arguments.blocks.BlockStateParser.parseForBlock(BuiltInRegistries.BLOCK,raw,true).blockState(); }
                catch (com.mojang.brigadier.exceptions.CommandSyntaxException ignored) {}
                out.printf("MARK\t%d\t%d\t%d\t%s%n",p.getX(),p.getY(),p.getZ(),key(state));
            }
        }
    }
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class); bind.setAccessible(true);
        for (var block : List.of(Blocks.STONE_BRICK_WALL, Blocks.MOSSY_STONE_BRICK_WALL))
            bind.invoke(block.builtInRegistryHolder(), List.of(BlockTags.WALLS));
        for (var state : Blocks.JIGSAW.getStateDefinition().getPossibleStates())
            for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                var result = state.mirror(mirror).rotate(rotation);
                out.printf("JSTATE\t%s\t%d\t%d\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n",
                    state.getValue(JigsawBlock.ORIENTATION).getSerializedName(), mirror.ordinal(), rotation.ordinal(),
                    result.getValue(JigsawBlock.ORIENTATION).getSerializedName(), state.hasBlockEntity(),
                    state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO),
                    state.getPostProcessPos(EmptyBlockGetter.INSTANCE, BlockPos.ZERO) == null,
                    state.useShapeForLightOcclusion(), net.minecraft.world.level.lighting.LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(),state),
                    net.minecraft.world.level.lighting.LightEngine.hasDifferentLightProperties(Blocks.STONE.defaultBlockState(),state),
                    net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state));
            }
        for (var state : Blocks.BARRIER.getStateDefinition().getPossibleStates())
            out.printf("BSTATE\t%s\t%d\t%d\t%s\t%s\t%s\t%s\t%s\t%s%n",
                state.getValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.WATERLOGGED),
                state.getLightDampening(), state.getLightEmission(), state.useShapeForLightOcclusion(),
                state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE,BlockPos.ZERO), state.hasBlockEntity(),
                state.getPostProcessPos(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)==null,
                net.minecraft.world.level.lighting.LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(),state),
                net.minecraft.world.level.lighting.LightEngine.hasDifferentLightProperties(Blocks.WATER.defaultBlockState(),state));
        for (var block : List.of(Blocks.WATER, Blocks.LAVA))
            for (var state : block.getStateDefinition().getPossibleStates())
                for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                    var result = state.mirror(mirror).rotate(rotation);
                    out.printf("LSTATE\t%s\t%d\t%d\t%d\t%d%n", block == Blocks.WATER ? "minecraft:water" : "minecraft:lava",
                        state.getValue(LiquidBlock.LEVEL), mirror.ordinal(), rotation.ordinal(), result.getValue(LiquidBlock.LEVEL));
                }
        for (var b : List.of(Blocks.STONE, Blocks.STONE_BRICKS, Blocks.MOSSY_STONE_BRICKS,
                Blocks.CRACKED_STONE_BRICKS, Blocks.CHISELED_STONE_BRICKS, Blocks.COBBLESTONE,
                Blocks.MOSSY_COBBLESTONE, Blocks.OBSIDIAN, Blocks.CRYING_OBSIDIAN, Blocks.GOLD_BLOCK,
                Blocks.NETHERRACK, Blocks.MAGMA_BLOCK, Blocks.BLACKSTONE, Blocks.POLISHED_BLACKSTONE,
                Blocks.POLISHED_BLACKSTONE_BRICKS, Blocks.CRACKED_POLISHED_BLACKSTONE_BRICKS,
                Blocks.CHISELED_POLISHED_BLACKSTONE, Blocks.IRON_BARS, Blocks.JUNGLE_LEAVES,
                Blocks.VINE, Blocks.CHEST, Blocks.LAVA, Blocks.AIR, Blocks.CAVE_AIR, Blocks.VOID_AIR,
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
                if (b == Blocks.LAVA && state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO))
                    throw new AssertionError("lava unexpectedly has a full collision shape " + state);
            }
        var root=new CompoundTag(); root.put("size",ints(4,2,4));
        var palette=new ListTag(); palette.add(NbtUtils.writeBlockState(Blocks.STONE.defaultBlockState()));
        palette.add(NbtUtils.writeBlockState(Blocks.IRON_BARS.defaultBlockState()));
        palette.add(NbtUtils.writeBlockState(Blocks.STONE_BRICK_STAIRS.defaultBlockState()));
        palette.add(NbtUtils.writeBlockState(Blocks.STONE_BRICK_STAIRS.defaultBlockState().setValue(StairBlock.FACING, Direction.EAST)));
        palette.add(NbtUtils.writeBlockState(Blocks.STONE_SLAB.defaultBlockState()));
        palette.add(NbtUtils.writeBlockState(Blocks.JIGSAW.defaultBlockState())); root.put("palette",palette);
        var blocks=new ListTag();
        for (int[] b:new int[][]{{0,0,0,0},{2,0,0,0},{1,0,0,1},{1,0,1,1},{3,1,1,1},
                {2,1,1,2},{2,1,0,3},{0,1,1,4},{1,1,2,5}}) {
            var tag=new CompoundTag(); tag.put("pos",ints(b[0],b[1],b[2])); tag.putInt("state",b[3]);
            if (b[3] == 5) { var nbt = new CompoundTag(); nbt.putString("final_state", "minecraft:netherrack"); tag.put("nbt",nbt); }
            blocks.add(tag);
        }
        root.put("blocks",blocks); root.put("entities",new ListTag());
        var template=new StructureTemplate(); template.load(BuiltInRegistries.BLOCK,root);
        var origin=new BlockPos(4,12,-6); var pivot=new BlockPos(2,0,2);
        for (var mirror:Mirror.values()) for (var rotation:Rotation.values()) {
            var states=new HashMap<BlockPos,BlockState>(); var events=new ArrayList<String>();
            var ticks = new ArrayList<String>();
            var attributes=(EnvironmentAttributeReader)Proxy.newProxyInstance(EnvironmentAttributeReader.class.getClassLoader(),new Class<?>[]{EnvironmentAttributeReader.class},
                (proxy,method,values)->switch(method.getName()) { case "getDimensionValue" -> false; default -> throw new UnsupportedOperationException(method.toString()); });
            var levelRandom=RandomSource.create(17L);
            var world=(ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},
                (proxy,m,a)->switch(m.getName()) {
                    case "getBlockState" -> states.getOrDefault((BlockPos)a[0],Blocks.AIR.defaultBlockState());
                    case "getFluidState" -> states.getOrDefault((BlockPos)a[0],Blocks.AIR.defaultBlockState()).getFluidState();
                    case "getRandom" -> levelRandom;
                    case "environmentAttributes" -> attributes;
                    case "scheduleTick" -> { ticks.add(BuiltInRegistries.FLUID.getKey((Fluid)a[1])+":"+a[2]); yield null; }
                    case "setBlock" -> { var p=((BlockPos)a[0]).immutable(); var s=(BlockState)a[1];
                        states.put(p,s); events.add("W\t"+p.getX()+"\t"+p.getY()+"\t"+p.getZ()+"\t"+key(s)+"\t"+a[2]); yield true; }
                    case "setBlockAndUpdate" -> { var p=((BlockPos)a[0]).immutable(); var s=(BlockState)a[1];
                        states.put(p,s); events.add("W\t"+p.getX()+"\t"+p.getY()+"\t"+p.getZ()+"\t"+key(s)+"\t3"); yield true; }
                    case "updateNeighborsAt" -> {var p=(BlockPos)a[0]; events.add("N\t"+p.getX()+"\t"+p.getY()+"\t"+p.getZ()); yield null;}
                    case "getBlockEntity" -> null;
                    default -> throw new UnsupportedOperationException(m.toString());
                });
            if (mirror == Mirror.NONE && rotation == Rotation.NONE) {
                for (var b : List.of(Blocks.DIRT, Blocks.STONE, Blocks.STONE_BRICKS, Blocks.MOSSY_STONE_BRICKS,
                        Blocks.CRACKED_STONE_BRICKS, Blocks.CHISELED_STONE_BRICKS, Blocks.COBBLESTONE,
                        Blocks.MOSSY_COBBLESTONE, Blocks.OBSIDIAN, Blocks.CRYING_OBSIDIAN, Blocks.GOLD_BLOCK,
                        Blocks.NETHERRACK, Blocks.MAGMA_BLOCK, Blocks.BLACKSTONE, Blocks.POLISHED_BLACKSTONE,
                        Blocks.POLISHED_BLACKSTONE_BRICKS, Blocks.CRACKED_POLISHED_BLACKSTONE_BRICKS,
                        Blocks.CHISELED_POLISHED_BLACKSTONE)) {
                    var s = b.defaultBlockState();
                    if (!s.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO))
                        throw new AssertionError("portal block is not collision-full " + s);
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
                for (int level = 0; level < 16; level++) for (var d : Direction.values())
                    for (var neighbor : List.of(Blocks.AIR.defaultBlockState(), Blocks.WATER.defaultBlockState())) {
                        ticks.clear();
                        var lava = Blocks.LAVA.defaultBlockState().setValue(LiquidBlock.LEVEL, level);
                        if (lava.updateShape(world, world, BlockPos.ZERO, d, BlockPos.ZERO.relative(d), neighbor, levelRandom) != lava)
                            throw new AssertionError("lava shape changed " + level);
                        var expectedTicks = level == 0 || neighbor.getFluidState().isSource()
                            ? List.of((level == 0 ? "minecraft:lava" : "minecraft:flowing_lava") + ":30") : List.of();
                        if (!ticks.equals(expectedTicks)) throw new AssertionError("lava tick mismatch " + level + " " + d + " " + ticks);
                    }
                if (!events.isEmpty()) throw new AssertionError("unexpected shape effects");
            }
            var settings=new StructurePlaceSettings().setMirror(mirror).setRotation(rotation).setRotationPivot(pivot)
                .setBoundingBox(new BoundingBox(-20,-20,-20,20,30,20)).setIgnoreEntities(true);
            var random=new WorldgenRandom(new XoroshiroRandomSource(42));
            if (!template.placeInWorld(world,origin,origin,settings,random,2)) throw new AssertionError("empty template");
            for (var marker : template.filterBlocks(origin,settings,Blocks.JIGSAW)) if (marker.nbt()!=null) {
                var state=net.minecraft.commands.arguments.blocks.BlockStateParser.parseForBlock(BuiltInRegistries.BLOCK,
                    marker.nbt().getStringOr("final_state","minecraft:air"),true).blockState();
                world.setBlockAndUpdate(marker.pos(),state);
            }
            out.printf("CASE\t%d\t%d\t%d%n",mirror.ordinal(),rotation.ordinal(),events.size());
            for (var e:events) out.println(e);
            out.printf("END\t%d%n",random.nextLong());
        }
        var names = new ArrayList<String>();
        for (int i=1;i<=10;i++) names.add("portal_"+i);
        for (int i=1;i<=3;i++) names.add("giant_portal_"+i);
        for (var name : names) {
            try (var stream = StructurePlacementOracle.class.getClassLoader().getResourceAsStream(
                    "data/minecraft/structure/ruined_portal/"+name+".nbt")) {
                if (stream == null) throw new AssertionError("missing " + name);
                var actualTemplate = new StructureTemplate();
                actualTemplate.load(BuiltInRegistries.BLOCK, NbtIo.readCompressed(stream, NbtAccounter.unlimitedHeap()));
                markers(out, name, actualTemplate, origin);
            }
        }
        var synthetic = new CompoundTag(); synthetic.put("size",ints(4,4,4));
        var syntheticPalette = new ListTag(); syntheticPalette.add(NbtUtils.writeBlockState(Blocks.JIGSAW.defaultBlockState())); synthetic.put("palette",syntheticPalette);
        var syntheticBlocks = new ListTag();
        int[][] positions = {{3,3,3},{1,1,1},{2,1,1},{0,0,0},{1,2,2}};
        String[] finals = {"minecraft:stone", "minecraft:stone[unknown=yes]", null, "NO_NBT", "minecraft:stone_brick_stairs[facing=east,half=top]"};
        for (int i=0;i<positions.length;i++) {
            var block=new CompoundTag(); block.put("pos",ints(positions[i])); block.putInt("state",0);
            if (!"NO_NBT".equals(finals[i])) { var nbt=new CompoundTag(); if (finals[i]!=null) nbt.putString("final_state",finals[i]); block.put("nbt",nbt); }
            syntheticBlocks.add(block);
        }
        synthetic.put("blocks",syntheticBlocks); synthetic.put("entities",new ListTag());
        var syntheticTemplate=new StructureTemplate(); syntheticTemplate.load(BuiltInRegistries.BLOCK,synthetic);
        markers(out,"synthetic",syntheticTemplate,origin);
    }
}
