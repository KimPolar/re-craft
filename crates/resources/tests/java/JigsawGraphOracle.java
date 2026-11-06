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
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.JigsawBlock;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.structure.PoolElementStructurePiece;
import net.minecraft.world.level.levelgen.structure.pools.*;
import net.minecraft.world.level.levelgen.structure.pools.alias.PoolAliasLookup;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.*;

public class JigsawGraphOracle {
    static StructureTemplate template(int sizeX, boolean end) throws Exception {
        var result = new StructureTemplate();
        Field sizeField=StructureTemplate.class.getDeclaredField("size"); sizeField.setAccessible(true);
        sizeField.set(result,new Vec3i(sizeX,3,3));
        var blocks = new ArrayList<StructureTemplate.StructureBlockInfo>();
        var positions = new BlockPos[]{new BlockPos(sizeX-1,1,1),new BlockPos(0,1,1),new BlockPos(1,1,0),new BlockPos(1,1,2)};
        var directions = new FrontAndTop[]{FrontAndTop.EAST_UP,FrontAndTop.WEST_UP,FrontAndTop.NORTH_UP,FrontAndTop.SOUTH_UP};
        int[] placement={3,0,2,1}, selection={0,2,0,1};
        for (int i=0;i<(end?1:4);i++) {
            var nbt=new CompoundTag();
            nbt.putString("name","example:door"); nbt.putString("target","example:door");
            nbt.putString("pool",end?"minecraft:empty":"example:children");
            nbt.putString("joint","aligned"); nbt.putInt("placement_priority",placement[i]); nbt.putInt("selection_priority",selection[i]);
            var state=Blocks.JIGSAW.defaultBlockState().setValue(JigsawBlock.ORIENTATION,directions[i]);
            blocks.add(new StructureTemplate.StructureBlockInfo(positions[i],state,nbt));
        }
        var palette=StructureTemplate.Palette.class.getDeclaredConstructor(List.class); palette.setAccessible(true);
        Field palettes=StructureTemplate.class.getDeclaredField("palettes"); palettes.setAccessible(true);
        palettes.set(result,new ArrayList<>(List.of(palette.newInstance(blocks))));
        return result;
    }
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field uf=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); uf.setAccessible(true); var unsafe=(sun.misc.Unsafe)uf.get(null);
        var manager=(StructureTemplateManager)unsafe.allocateInstance(StructureTemplateManager.class);
        Field repository=StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var templates=new HashMap<Identifier,Optional<StructureTemplate>>();
        templates.put(Identifier.parse("example:a"),Optional.of(template(3,false)));
        templates.put(Identifier.parse("example:b"),Optional.of(template(2,false)));
        templates.put(Identifier.parse("example:end"),Optional.of(template(1,true)));
        repository.set(manager,templates);
        var projection=StructureTemplatePool.Projection.RIGID;
        var a=StructurePoolElement.single("example:a").apply(projection);
        var b=StructurePoolElement.single("example:b").apply(projection);
        var end=StructurePoolElement.single("example:end").apply(projection);
        var registry=new MappedRegistry<StructureTemplatePool>(Registries.TEMPLATE_POOL,Lifecycle.stable());
        var empty=registry.register(Pools.EMPTY,new StructureTemplatePool(null,List.of()),RegistrationInfo.BUILT_IN);
        var endKey=ResourceKey.create(Registries.TEMPLATE_POOL,Identifier.parse("example:terminators"));
        var endPool=registry.register(endKey,new StructureTemplatePool(empty,List.of(Pair.of(end,1))),RegistrationInfo.BUILT_IN);
        var childKey=ResourceKey.create(Registries.TEMPLATE_POOL,Identifier.parse("example:children"));
        registry.register(childKey,new StructureTemplatePool(endPool,List.of(Pair.of(a,2),Pair.of(b,1),Pair.of(EmptyPoolElement.INSTANCE,1))),RegistrationInfo.BUILT_IN);
        registry.freeze();
        var method=Arrays.stream(JigsawPlacement.class.getDeclaredMethods()).filter(m -> m.getName().equals("addPieces") && m.getParameterCount()==13).findFirst().orElseThrow();
        method.setAccessible(true);
        for (int depth : new int[]{1,2,3}) for (long seed : new long[]{0,1,2,3,42,-1,Long.MIN_VALUE,Long.MAX_VALUE}) {
            var pos=new BlockPos(0,30,0); var bounds=a.getBoundingBox(manager,pos,Rotation.NONE);
            var root=new PoolElementStructurePiece(manager,a,pos,1,Rotation.NONE,bounds,LiquidSettings.APPLY_WATERLOGGING);
            var pieces=new ArrayList<PoolElementStructurePiece>(); pieces.add(root);
            var random=new LegacyRandomSource(seed);
            var free=Shapes.join(Shapes.create(new AABB(-9,20,-9,10,41,10)),Shapes.create(AABB.of(bounds)),BooleanOp.ONLY_FIRST);
            method.invoke(null,null,depth,false,null,manager,null,random,registry,root,pieces,free,PoolAliasLookup.EMPTY,LiquidSettings.APPLY_WATERLOGGING);
            out.printf("C\t%d\t%d\t%d\t%d%n",depth,seed,pieces.size(),random.nextLong());
            for (var piece:pieces) {
                var p=piece.getPosition(); var bnd=piece.getBoundingBox();
                var id=((SinglePoolElement)piece.getElement()).getTemplateLocation();
                out.printf("P\t%s\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d%n",id,piece.getRotation().ordinal(),p.getX(),p.getY(),p.getZ(),bnd.minX(),bnd.minY(),bnd.minZ(),bnd.maxX(),bnd.maxY(),bnd.maxZ(),piece.getGroundLevelDelta(),piece.getJunctions().size());
                for (var j:piece.getJunctions()) out.printf("J\t%d\t%d\t%d\t%d\t%s%n",j.getSourceX(),j.getSourceGroundY(),j.getSourceZ(),j.getDeltaY(),j.getDestProjection().getSerializedName());
            }
        }
    }
}
