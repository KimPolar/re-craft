import com.mojang.datafixers.util.Pair;
import com.mojang.serialization.Lifecycle;
import java.lang.reflect.Field;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.worldgen.Pools;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.util.SequencedPriorityIterator;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.JigsawBlock;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.PoolElementStructurePiece;
import net.minecraft.world.level.levelgen.structure.pools.*;
import net.minecraft.world.level.levelgen.structure.pools.alias.PoolAliasLookup;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.*;
import org.apache.commons.lang3.mutable.MutableObject;

public class JigsawPlacementOracle {
    static class FlatGenerator extends net.minecraft.world.level.chunk.ChunkGenerator {
        int height;
        FlatGenerator() { super(null); }
        @Override protected com.mojang.serialization.MapCodec<? extends net.minecraft.world.level.chunk.ChunkGenerator> codec() { throw new UnsupportedOperationException(); }
        @Override public void spawnOriginalMobs(net.minecraft.server.level.WorldGenRegion region) { throw new UnsupportedOperationException(); }
        @Override public int getGenDepth() { return 0; }
        @Override public java.util.concurrent.CompletableFuture<net.minecraft.world.level.chunk.ChunkAccess> buildTerrain(net.minecraft.world.level.chunk.ChunkAccess chunk, net.minecraft.world.level.levelgen.blending.Blender blender, net.minecraft.world.level.levelgen.RandomState random, net.minecraft.world.level.StructureManager structures, net.minecraft.world.level.biome.BiomeManager biomes, net.minecraft.server.level.WorldGenRegion region, Set<Holder<net.minecraft.world.level.biome.Biome>> samples) { throw new UnsupportedOperationException(); }
        @Override public int getSeaLevel() { return 0; }
        @Override public int getMinY() { return 0; }
        @Override public int getBaseHeight(int x, int z, net.minecraft.world.level.levelgen.Heightmap.Types type, net.minecraft.world.level.LevelHeightAccessor height, net.minecraft.world.level.levelgen.RandomState random) { return 42; }
        @Override public net.minecraft.world.level.NoiseColumn getBaseColumn(int x, int z, net.minecraft.world.level.LevelHeightAccessor height, net.minecraft.world.level.levelgen.RandomState random) { throw new UnsupportedOperationException(); }
        @Override public void addDebugScreenInfo(List<String> info, net.minecraft.world.level.levelgen.RandomState random, BlockPos pos, net.minecraft.world.level.levelgen.densityfunction.SamplerContext context) { throw new UnsupportedOperationException(); }
        @Override public int getFirstFreeHeight(int x, int z, net.minecraft.world.level.levelgen.Heightmap.Types type, net.minecraft.world.level.LevelHeightAccessor height, net.minecraft.world.level.levelgen.RandomState random) { return this.height; }
    }
    static StructureTemplate template(int[] size, BlockPos position, FrontAndTop orientation, String name, String target, String pool) throws Exception {
        var result = new StructureTemplate();
        Field sizeField = StructureTemplate.class.getDeclaredField("size"); sizeField.setAccessible(true);
        sizeField.set(result, new Vec3i(size[0], size[1], size[2]));
        var nbt = new CompoundTag();
        nbt.putString("name", name); nbt.putString("target", target); nbt.putString("pool", pool);
        nbt.putString("joint", "aligned");
        var state = Blocks.JIGSAW.defaultBlockState().setValue(JigsawBlock.ORIENTATION, orientation);
        var info = new StructureTemplate.StructureBlockInfo(position, state, nbt);
        var constructor = StructureTemplate.Palette.class.getDeclaredConstructor(List.class); constructor.setAccessible(true);
        Field palettes = StructureTemplate.class.getDeclaredField("palettes"); palettes.setAccessible(true);
        palettes.set(result, new ArrayList<>(List.of(constructor.newInstance(List.of(info)))));
        return result;
    }

    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();

        var root = new BoundingBox(-5,-5,-5,5,5,5);
        var initial = new BoundingBox(-1,-1,-1,1,1,1);
        VoxelShape free = Shapes.join(Shapes.create(AABB.of(root)), Shapes.create(AABB.of(initial)), BooleanOp.ONLY_FIRST);
        var sequence = new Random(193);
        for (int i = 0; i < 240; i++) {
            int x = sequence.nextInt(17)-8, y = sequence.nextInt(17)-8, z = sequence.nextInt(17)-8;
            int sx = sequence.nextInt(3), sy = sequence.nextInt(3), sz = sequence.nextInt(3);
            var box = new BoundingBox(x,y,z,x+sx,y+sy,z+sz);
            boolean fits = !Shapes.joinIsNotEmpty(free, Shapes.create(AABB.of(box).deflate(0.25)), BooleanOp.ONLY_SECOND);
            out.printf("B\t%d\t%d\t%d\t%d\t%d\t%d\t%s%n",x,y,z,sx,sy,sz,fits);
            if (fits) free = Shapes.joinUnoptimized(free, Shapes.create(AABB.of(box)), BooleanOp.ONLY_FIRST);
        }
        var queue = new SequencedPriorityIterator<String>();
        queue.add("a", Integer.MIN_VALUE); queue.add("b", 2); queue.add("c", 2);
        var order = new StringBuilder(); order.append(queue.next());
        queue.add("d", 3); queue.add("e", 2); queue.add("f", -1);
        while (queue.hasNext()) order.append(queue.next());
        out.printf("Q\t%s%n",order);

        Field unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)unsafeField.get(null);
        var generator = (FlatGenerator)unsafe.allocateInstance(FlatGenerator.class);
        var manager = (StructureTemplateManager)unsafe.allocateInstance(StructureTemplateManager.class);
        var repository = StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var parentId = Identifier.parse("example:parent"); var childId = Identifier.parse("example:child");
        var templates = new HashMap<Identifier,Optional<StructureTemplate>>();
        repository.set(manager, templates);
        var registry = new MappedRegistry<StructureTemplatePool>(Registries.TEMPLATE_POOL, Lifecycle.stable());
        var empty = registry.register(Pools.EMPTY, new StructureTemplatePool(null,List.of()), RegistrationInfo.BUILT_IN);
        var childElement = StructurePoolElement.single("example:child").apply(StructureTemplatePool.Projection.RIGID);
        var childKey = ResourceKey.create(Registries.TEMPLATE_POOL, Identifier.parse("example:children"));
        registry.register(childKey,new StructureTemplatePool(empty,List.of(Pair.of(childElement,1))),RegistrationInfo.BUILT_IN);
        registry.freeze();
        var parentElement = StructurePoolElement.single("example:parent").apply(StructureTemplatePool.Projection.RIGID);
        var placerClass = Class.forName("net.minecraft.world.level.levelgen.structure.pools.JigsawPlacement$Placer");
        var ctor = placerClass.getDeclaredConstructors()[0]; ctor.setAccessible(true);
        var method = Arrays.stream(placerClass.getDeclaredMethods()).filter(m -> m.getName().equals("tryPlacingChildren")).findFirst().orElseThrow(); method.setAccessible(true);
        var fronts = new FrontAndTop[]{FrontAndTop.WEST_UP,FrontAndTop.NORTH_UP,FrontAndTop.EAST_UP,FrontAndTop.SOUTH_UP,FrontAndTop.DOWN_NORTH,FrontAndTop.UP_NORTH};
        var localPositions = new BlockPos[]{new BlockPos(0,1,2),new BlockPos(1,1,0),new BlockPos(3,1,2),new BlockPos(1,1,4),new BlockPos(1,0,2),new BlockPos(1,2,2)};
        int[] parentHeights = {2,0,4};
        for (int mode=0; mode<fronts.length; mode++) for (int projection=0; projection<4; projection++) for (int heightCase=0; heightCase<parentHeights.length; heightCase++) for (long seed : new long[]{0,1,-1,42,Long.MIN_VALUE,Long.MAX_VALUE}) {
            var parentPos = mode<4 ? new BlockPos(4,parentHeights[heightCase],2) : mode==4 ? new BlockPos(2,4,2) : new BlockPos(2,0,2);
            var parentFront = mode<4 ? FrontAndTop.EAST_UP : mode==4 ? FrontAndTop.UP_NORTH : FrontAndTop.DOWN_NORTH;
            generator.height = mode<4 ? 42 : mode==4 ? 45 : 39;
            templates.put(parentId, Optional.of(template(new int[]{5,5,5}, parentPos, parentFront, "example:parent", "example:door", "example:children")));
            templates.put(childId, Optional.of(template(new int[]{4,3,5}, localPositions[mode], fronts[mode], "example:door", "minecraft:empty", "minecraft:empty")));
            parentElement.setProjection((projection & 1)==0 ? StructureTemplatePool.Projection.RIGID : StructureTemplatePool.Projection.TERRAIN_MATCHING);
            childElement.setProjection((projection & 2)==0 ? StructureTemplatePool.Projection.RIGID : StructureTemplatePool.Projection.TERRAIN_MATCHING);
            var position = new BlockPos(10,40,20);
            var bounds = parentElement.getBoundingBox(manager, position, Rotation.NONE);
            var parent = new PoolElementStructurePiece(manager,parentElement,position,1,Rotation.NONE,bounds,LiquidSettings.APPLY_WATERLOGGING);
            var pieces = new ArrayList<PoolElementStructurePiece>(); pieces.add(parent);
            var random = new LegacyRandomSource(seed);
            var placer = ctor.newInstance(registry,1,generator,manager,pieces,random);
            var space = new MutableObject<VoxelShape>(Shapes.join(Shapes.create(new AABB(-100,-100,-100,100,100,100)),Shapes.create(AABB.of(bounds)),BooleanOp.ONLY_FIRST));
            method.invoke(placer,parent,space,0,false,null,null,PoolAliasLookup.EMPTY,LiquidSettings.APPLY_WATERLOGGING);
            if (pieces.size()!=2) throw new AssertionError("expected one child: "+pieces.size());
            var child = pieces.get(1); var b=child.getBoundingBox(); var p=child.getPosition();
            var j=parent.getJunctions().get(0); var k=child.getJunctions().get(0);
            out.printf("G\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d%n",
                mode,projection,heightCase,seed,child.getRotation().ordinal(),p.getX(),p.getY(),p.getZ(),b.minX(),b.minY(),b.minZ(),b.maxX(),b.maxY(),b.maxZ(),child.getGroundLevelDelta(),
                j.getSourceX(),j.getSourceGroundY(),j.getSourceZ(),j.getDeltaY(),k.getSourceX(),k.getSourceGroundY(),k.getSourceZ(),k.getDeltaY(),random.nextLong(),parent.getJunctions().size(),child.getJunctions().size());
        }
    }
}
